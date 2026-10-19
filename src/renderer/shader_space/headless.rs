use std::{collections::HashMap, io::Write, path::Path};

use anyhow::{Result, anyhow, bail};
use rust_wgpu_fiber::HeadlessRenderer;
use rust_wgpu_fiber::HeadlessRendererConfig;
use rust_wgpu_fiber::wgpu::TextureFormat;
use rust_wgpu_fiber::shader_space::{
    PASS_CAPTURE_OUTPUT_TEXTURE_NAME, PassCaptureMode, PassCaptureRequest,
};

use crate::asset_store::AssetStore;
use crate::dsl::SceneDSL;
use crate::frame_freeze::{
    FrameFreezeCase, apply_param_values, apply_state_param_values, fingerprint_hex, validate_case,
};
use crate::profile::{self, ProfileAccumulator, ProfileRunConfig, ProfileWriter};
use crate::state_machine::OverrideKey;
use crate::ui::resource_tree::ResourceSnapshot;

use super::api::{
    ShaderSpaceBuildOptions, ShaderSpaceBuildResult, ShaderSpaceBuilder,
    ShaderSpacePresentationMode,
};

pub type HeadlessRuntimeOverrides = HashMap<OverrideKey, serde_json::Value>;

fn build_headless_shader_space(
    renderer: &HeadlessRenderer,
    scene: &SceneDSL,
    asset_store: Option<&AssetStore>,
    runtime_overrides: Option<&HeadlessRuntimeOverrides>,
    frame_freeze: Option<&FrameFreezeCase>,
) -> Result<ShaderSpaceBuildResult> {
    if runtime_overrides.is_some() && frame_freeze.is_some() {
        bail!("headless runtime overrides and frame freeze are mutually exclusive");
    }
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

    if let Some(frame_freeze) = frame_freeze {
        validate_case(frame_freeze)?;
        if result.scene_output_texture.as_str() != frame_freeze.payload.scene_output_texture_name {
            bail!(
                "freeze output texture mismatch: archive={} freeze={}",
                result.scene_output_texture,
                frame_freeze.payload.scene_output_texture_name
            );
        }
        let archive_fingerprint = fingerprint_hex(&result.pipeline_signature);
        if archive_fingerprint != frame_freeze.topology_fingerprint {
            eprintln!(
                "[frame-freeze] pipeline fingerprint changed; continuing with semantic replay validation: archive={} freeze={}",
                archive_fingerprint, frame_freeze.topology_fingerprint
            );
        }
        let prepared_scene = result
            .prepared_scene
            .as_mut()
            .ok_or_else(|| anyhow!("frame freeze requires a prepared scene"))?;
        apply_state_param_values(prepared_scene, &frame_freeze.payload.state_params);
        apply_param_values(prepared_scene, &frame_freeze.payload.node_params)?;
        crate::renderer::graph_uniforms::apply_graph_uniform_updates(
            &mut result.pass_bindings,
            &mut result.shader_space,
            prepared_scene,
        )?;
        if !result.gaussian_blur_bundles.is_empty() {
            super::dynamic_gaussian_blur::update_dynamic_gaussian_blur_bundles(
                prepared_scene,
                &mut result.shader_space,
                &mut result.pass_bindings,
                &mut result.gaussian_blur_bundles,
                &HashMap::new(),
            )?;
        }
        result.pipeline_signature =
            crate::renderer::graph_uniforms::compute_pipeline_signature_for_pass_bindings(
                prepared_scene,
                &result.pass_bindings,
            );
        for pass in &mut result.pass_bindings {
            let mut params = pass.base_params;
            params.time = frame_freeze.payload.scene_time_seconds;
            crate::renderer::update_pass_params(&result.shader_space, pass, &params)?;
        }
        return Ok(result);
    }

    let Some(runtime_overrides) = runtime_overrides else {
        return Ok(result);
    };
    let prepared_scene = result
        .prepared_scene
        .as_mut()
        .ok_or_else(|| anyhow!("headless runtime overrides require a prepared scene"))?;
    crate::state_machine::apply_overrides(prepared_scene, runtime_overrides);
    crate::renderer::graph_uniforms::apply_graph_uniform_updates(
        &mut result.pass_bindings,
        &mut result.shader_space,
        prepared_scene,
    )?;
    if !result.gaussian_blur_bundles.is_empty() {
        super::dynamic_gaussian_blur::update_dynamic_gaussian_blur_bundles(
            prepared_scene,
            &mut result.shader_space,
            &mut result.pass_bindings,
            &mut result.gaussian_blur_bundles,
            &HashMap::new(),
        )?;
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadlessOutputKind {
    Png,
    Exr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadlessTextureOutputKind {
    Exr,
    RawRgba16F,
    RawRgba32F,
}

fn route_headless_output(format: TextureFormat, output_path: &Path) -> Result<HeadlessOutputKind> {
    match format {
        TextureFormat::Rgba16Float | TextureFormat::Rgba32Float => {
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
            "headless file export unsupported for scene output format {other:?}; supported: Rgba8Unorm/Rgba8UnormSrgb (png), Rgba16Float/Rgba32Float (exr)"
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
        (TextureFormat::Rgba32Float, Some("rgba32f")) => Ok(HeadlessTextureOutputKind::RawRgba32F),
        (TextureFormat::Rgba32Float, Some("exr")) => Ok(HeadlessTextureOutputKind::Exr),
        (TextureFormat::Rgba32Float, _) => bail!(
            "RGBA32Float texture export requires .rgba32f (exact float bits) or .exr; got {}",
            output_path.display()
        ),
        (TextureFormat::Rgba16Float, _) => bail!(
            "RGBA16Float texture export requires .rgba16f (exact half bits) or .exr; got {}",
            output_path.display()
        ),
        (other, _) => bail!(
            "headless texture export unsupported for format {other:?}; expected Rgba16Float or Rgba32Float"
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
    save_float_rgba_raw(output_path, width, height, channels, false)
}

fn save_rgba32f_raw(output_path: &Path, width: u32, height: u32, channels: &[f32]) -> Result<()> {
    save_float_rgba_raw(output_path, width, height, channels, true)
}

fn save_float_rgba_raw(
    output_path: &Path, width: u32, height: u32, channels: &[f32], full_precision: bool,
) -> Result<()> {
    let expected_len = width as usize * height as usize * 4;
    if channels.len() != expected_len {
        bail!(
            "invalid float RGBA readback shape: expected {expected_len} channels, got {}",
            channels.len()
        );
    }

    let file = std::fs::File::create(output_path)
        .map_err(|error| anyhow!("failed to create {}: {error}", output_path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    writer.write_all(if full_precision { b"NF32" } else { b"NF16" })?;
    writer.write_all(&1u32.to_le_bytes())?;
    writer.write_all(&width.to_le_bytes())?;
    writer.write_all(&height.to_le_bytes())?;
    writer.write_all(&4u32.to_le_bytes())?;
    for value in channels {
        if full_precision {
            writer.write_all(&value.to_le_bytes())?;
        } else {
            writer.write_all(&half::f16::from_f32(*value).to_bits().to_le_bytes())?;
        }
    }
    writer.flush()?;
    Ok(())
}

pub fn render_scene_to_file_headless(
    scene: &SceneDSL,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
) -> Result<()> {
    render_scene_to_file_headless_with_runtime_overrides(scene, output_path, asset_store, None)
}

pub fn render_scene_to_file_headless_with_runtime_overrides(
    scene: &SceneDSL,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    runtime_overrides: Option<&HeadlessRuntimeOverrides>,
) -> Result<()> {
    render_scene_to_file_headless_with_runtime(
        scene,
        output_path,
        asset_store,
        runtime_overrides,
        None,
    )
}

pub fn render_scene_to_file_headless_with_frame_freeze(
    scene: &SceneDSL,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    frame_freeze: &FrameFreezeCase,
) -> Result<()> {
    render_scene_to_file_headless_with_runtime(
        scene,
        output_path,
        asset_store,
        None,
        Some(frame_freeze),
    )
}

fn render_scene_to_file_headless_with_runtime(
    scene: &SceneDSL,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    runtime_overrides: Option<&HeadlessRuntimeOverrides>,
    frame_freeze: Option<&FrameFreezeCase>,
) -> Result<()> {
    let output_path = output_path.as_ref();
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|e| anyhow!("failed to create headless renderer: {e}"))?;

    // Build the default GPU graph first, matching the interactive app. State
    // values are uploaded only after resource allocation and pipeline build.
    let result = build_headless_shader_space(
        &renderer,
        scene,
        asset_store,
        runtime_overrides,
        frame_freeze,
    )?;

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
    render_scene_texture_to_file_headless_with_runtime_overrides(
        scene,
        texture_name,
        output_path,
        asset_store,
        None,
    )
}

pub fn render_scene_texture_to_file_headless_with_runtime_overrides(
    scene: &SceneDSL,
    texture_name: &str,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    runtime_overrides: Option<&HeadlessRuntimeOverrides>,
) -> Result<()> {
    render_scene_texture_to_file_headless_with_runtime(
        scene,
        texture_name,
        output_path,
        asset_store,
        runtime_overrides,
        None,
    )
}

pub fn render_scene_texture_to_file_headless_with_frame_freeze(
    scene: &SceneDSL,
    texture_name: &str,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    frame_freeze: &FrameFreezeCase,
) -> Result<()> {
    render_scene_texture_to_file_headless_with_runtime(
        scene,
        texture_name,
        output_path,
        asset_store,
        None,
        Some(frame_freeze),
    )
}

fn render_scene_texture_to_file_headless_with_runtime(
    scene: &SceneDSL,
    texture_name: &str,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    runtime_overrides: Option<&HeadlessRuntimeOverrides>,
    frame_freeze: Option<&FrameFreezeCase>,
) -> Result<()> {
    let output_path = output_path.as_ref();
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|error| anyhow!("failed to create headless renderer: {error}"))?;
    let result = build_headless_shader_space(
        &renderer,
        scene,
        asset_store,
        runtime_overrides,
        frame_freeze,
    )?;
    result.shader_space.render();

    let texture_info = result
        .shader_space
        .texture_info(texture_name)
        .ok_or_else(|| anyhow!("named headless texture not found: {texture_name}"))?;
    match route_headless_texture_output(texture_info.format, output_path)? {
        HeadlessTextureOutputKind::RawRgba32F => {
            let image = result.shader_space.read_texture_rgba32f(texture_name)
                .map_err(|error| anyhow!("failed to read RGBA32F texture {texture_name}: {error}"))?;
            save_rgba32f_raw(output_path, image.width, image.height, &image.channels)?;
        }
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
    render_scene_pass_to_file_headless_with_runtime_overrides(
        scene,
        pass_name,
        output_path,
        asset_store,
        None,
    )
}

pub fn render_scene_pass_to_file_headless_with_runtime_overrides(
    scene: &SceneDSL,
    pass_name: &str,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    runtime_overrides: Option<&HeadlessRuntimeOverrides>,
) -> Result<()> {
    render_scene_pass_to_file_headless_with_runtime(
        scene,
        pass_name,
        output_path,
        asset_store,
        runtime_overrides,
        None,
    )
}

pub fn render_scene_pass_to_file_headless_with_frame_freeze(
    scene: &SceneDSL,
    pass_name: &str,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    frame_freeze: &FrameFreezeCase,
) -> Result<()> {
    render_scene_pass_to_file_headless_with_runtime(
        scene,
        pass_name,
        output_path,
        asset_store,
        None,
        Some(frame_freeze),
    )
}

fn render_scene_pass_to_file_headless_with_runtime(
    scene: &SceneDSL,
    pass_name: &str,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    runtime_overrides: Option<&HeadlessRuntimeOverrides>,
    frame_freeze: Option<&FrameFreezeCase>,
) -> Result<()> {
    let output_path = output_path.as_ref();
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|error| anyhow!("failed to create headless renderer: {error}"))?;
    let mut result = build_headless_shader_space(
        &renderer,
        scene,
        asset_store,
        runtime_overrides,
        frame_freeze,
    )?;
    let request = PassCaptureRequest::new(pass_name, PassCaptureMode::Solo);
    let capture = result
        .shader_space
        .prepare_pass_capture(&request)
        .map_err(|error| anyhow!("cannot capture pass {pass_name}: {error}"))?;
    let _ = result
        .shader_space
        .render_profiled_with_pass_capture(true, &request);

    match route_headless_texture_output(capture.format, output_path)? {
        HeadlessTextureOutputKind::RawRgba32F => {
            let image = result.shader_space.read_texture_rgba32f(PASS_CAPTURE_OUTPUT_TEXTURE_NAME)
                .map_err(|error| anyhow!("failed to read RGBA32F pass capture {pass_name}: {error}"))?;
            save_rgba32f_raw(output_path, image.width, image.height, &image.channels)?;
        }
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
    render_scene_to_file_headless_profiled_with_runtime_overrides(
        scene,
        output_path,
        asset_store,
        profile_config,
        writer,
        None,
    )
}

pub fn render_scene_to_file_headless_profiled_with_runtime_overrides(
    scene: &SceneDSL,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    profile_config: &ProfileRunConfig,
    writer: &mut ProfileWriter,
    runtime_overrides: Option<&HeadlessRuntimeOverrides>,
) -> Result<()> {
    let output_path = output_path.as_ref();
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|e| anyhow!("failed to create headless renderer: {e}"))?;

    let result =
        build_headless_shader_space(&renderer, scene, asset_store, runtime_overrides, None)?;
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
    fn frame_freeze_reuses_the_existing_headless_builder() {
        let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/render/editor-examples/graph-rectangle/scene.nforge");
        let Ok((scene, store)) = crate::asset_store::load_from_nforge(&archive) else {
            return;
        };
        let Ok(renderer) = HeadlessRenderer::new(HeadlessRendererConfig::default()) else {
            return;
        };
        if renderer.adapter.get_info().backend == rust_wgpu_fiber::wgpu::Backend::Noop {
            return;
        }
        let base =
            build_headless_shader_space(&renderer, &scene, Some(&store), None, None).unwrap();
        let mut node_params = crate::frame_freeze::collect_scene_params(
            base.prepared_scene.as_ref().expect("prepared scene"),
        );
        let changed = node_params
            .iter_mut()
            .find(|value| value.param_name == "value" && value.value.as_f64().is_some());
        let Some(changed) = changed else {
            return;
        };
        changed.value = serde_json::json!(0.375);
        let expected_node_id = changed.node_id.clone();
        let mut diagnostic_only_fingerprint = base.pipeline_signature;
        diagnostic_only_fingerprint[0] ^= 0xff;
        let case = FrameFreezeCase {
            case_id: "freeze-test".into(),
            name: "Freeze test".into(),
            captured_at: "1770000000000".into(),
            topology_fingerprint: fingerprint_hex(&diagnostic_only_fingerprint),
            payload: crate::frame_freeze::FrameFreezeV1 {
                schema_version: crate::frame_freeze::FRAME_FREEZE_SCHEMA_VERSION,
                scene_output_texture_name: base.scene_output_texture.as_str().to_string(),
                scene_time_seconds: 1.5,
                state_params: Vec::new(),
                node_params,
            },
        };
        drop(base);

        let replay =
            build_headless_shader_space(&renderer, &scene, Some(&store), None, Some(&case))
                .unwrap();
        let replay_scene = replay
            .prepared_scene
            .as_ref()
            .expect("prepared replay scene");
        assert_eq!(
            replay_scene
                .nodes
                .iter()
                .find(|node| node.id == expected_node_id)
                .and_then(|node| node.params.get("value")),
            Some(&serde_json::json!(0.375))
        );
    }

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
    fn image_raw_rgba32f_export_keeps_bits_and_rejects_half_precision_extension() {
        assert!(route_headless_texture_output(
            TextureFormat::Rgba32Float, Path::new("image.rgba16f")
        ).is_err());
        assert_eq!(route_headless_texture_output(
            TextureFormat::Rgba32Float, Path::new("image.rgba32f")
        ).unwrap(), HeadlessTextureOutputKind::RawRgba32F);
        let path = std::env::temp_dir().join(format!("image-precision-{}.rgba32f", std::process::id()));
        let values = [-0.0f32, -0.25, 100_000.0, 1.5, 0.500_123_4, 1e-30, 1e20, 0.5];
        save_rgba32f_raw(&path, 2, 1, &values).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"NF32");
        let bits: Vec<_> = bytes[20..].chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap())).collect();
        assert_eq!(bits, values.map(f32::to_bits));
        std::fs::remove_file(path).unwrap();
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
