use std::path::Path;

use anyhow::{Result, anyhow};
use rust_wgpu_fiber::HeadlessRenderer;
use rust_wgpu_fiber::HeadlessRendererConfig;

use crate::asset_store::AssetStore;
use crate::dsl::SceneDSL;

use super::api::{ShaderSpaceBuildOptions, ShaderSpaceBuilder};

pub fn render_scene_to_png_headless(
    scene: &SceneDSL,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
) -> Result<()> {
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|e| anyhow!("failed to create headless renderer: {e}"))?;

    let mut builder = ShaderSpaceBuilder::new(renderer.device.clone(), renderer.queue.clone())
        .with_adapter(renderer.adapter.clone())
        .with_options(ShaderSpaceBuildOptions::default());
    if let Some(store) = asset_store {
        builder = builder.with_asset_store(store.clone());
    }
    let result = builder.build(scene)?;

    result.shader_space.render();
    result
        .shader_space
        .save_texture_png(result.scene_output_texture.as_str(), output_path)
        .map_err(|e| anyhow!("failed to save png: {e}"))?;
    Ok(())
}
