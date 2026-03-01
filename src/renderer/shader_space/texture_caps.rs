//! Texture capability validation.
//!
//! Before registering textures with `ShaderSpace`, this module validates that
//! every declared texture format supports the usages required by the render
//! passes that sample or blend into it.

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use image::DynamicImage;
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, TextureFormat},
    pool::texture_pool::TextureSpec as FiberTextureSpec,
};

use super::pass_spec::{RenderPassSpec, TextureCapabilityRequirement};

pub(crate) fn effective_texture_format_features(
    format: TextureFormat,
    device_features: wgpu::Features,
    adapter: Option<&wgpu::Adapter>,
) -> wgpu::TextureFormatFeatures {
    if device_features.contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES) {
        if let Some(adapter) = adapter {
            return adapter.get_texture_format_features(format);
        }
    }
    format.guaranteed_format_features(device_features)
}

pub(crate) fn image_texture_wgpu_format(image: &DynamicImage, srgb: bool) -> Result<TextureFormat> {
    let base_format = match image.color() {
        image::ColorType::L8 => TextureFormat::R8Unorm,
        image::ColorType::La8 => TextureFormat::Rg8Unorm,
        image::ColorType::Rgb8 => TextureFormat::Rgba8Unorm,
        image::ColorType::Rgba8 => TextureFormat::Rgba8Unorm,

        image::ColorType::L16 => TextureFormat::R16Unorm,
        image::ColorType::La16 => TextureFormat::Rg16Unorm,
        image::ColorType::Rgb16 => TextureFormat::Rgba16Unorm,
        image::ColorType::Rgba16 => TextureFormat::Rgba16Unorm,

        image::ColorType::Rgb32F => TextureFormat::Rgba32Float,
        image::ColorType::Rgba32F => TextureFormat::Rgba32Float,
        other => bail!("unsupported image color type for GPU texture format: {other:?}"),
    };

    Ok(if srgb {
        base_format.add_srgb_suffix()
    } else {
        base_format
    })
}

fn blend_state_requires_blendable(blend_state: wgpu::BlendState) -> bool {
    format!("{blend_state:?}") != format!("{:?}", wgpu::BlendState::REPLACE)
}

pub(crate) fn collect_texture_capability_requirements(
    texture_specs: &[FiberTextureSpec],
    render_pass_specs: &[RenderPassSpec],
    prepass_texture_samples: &[(String, ResourceName)],
) -> Result<Vec<TextureCapabilityRequirement>> {
    let mut requirements_by_name: HashMap<ResourceName, TextureCapabilityRequirement> =
        HashMap::new();

    for spec in texture_specs {
        let (name, format, usage, sample_count) = match spec {
            FiberTextureSpec::Texture {
                name,
                format,
                usage,
                sample_count,
                ..
            } => (name.clone(), *format, *usage, (*sample_count).max(1)),
            FiberTextureSpec::Image {
                name,
                image,
                usage,
                srgb,
            } => (
                name.clone(),
                image_texture_wgpu_format(image.as_ref(), *srgb)?,
                *usage,
                1,
            ),
        };

        if let Some(existing) = requirements_by_name.get_mut(&name) {
            if existing.format != format || existing.sample_count != sample_count {
                bail!(
                    "texture '{}' has conflicting declarations: first format={:?} sample_count={}, later format={:?} sample_count={}",
                    name,
                    existing.format,
                    existing.sample_count,
                    format,
                    sample_count
                );
            }
            existing.usage |= usage;
        } else {
            requirements_by_name.insert(
                name.clone(),
                TextureCapabilityRequirement {
                    name,
                    format,
                    usage,
                    sample_count,
                    sampled_by_passes: Vec::new(),
                    blend_target_passes: Vec::new(),
                },
            );
        }
    }

    for pass in render_pass_specs {
        for binding in &pass.texture_bindings {
            let req =
                requirements_by_name
                    .get_mut(&binding.texture)
                    .ok_or_else(|| {
                        anyhow!(
                            "internal texture capability validation error: pass '{}' samples undeclared texture '{}'",
                            pass.pass_id,
                            binding.texture
                        )
                    })?;
            req.sampled_by_passes.push(pass.pass_id.clone());
        }

        if blend_state_requires_blendable(pass.blend_state) {
            let req = requirements_by_name
                .get_mut(&pass.target_texture)
                .ok_or_else(|| {
                    anyhow!(
                        "internal texture capability validation error: pass '{}' targets undeclared texture '{}'",
                        pass.pass_id,
                        pass.target_texture
                    )
                })?;
            req.blend_target_passes.push(pass.pass_id.clone());
        }
    }

    for (pass_id, texture_name) in prepass_texture_samples {
        let req = requirements_by_name
            .get_mut(texture_name)
            .ok_or_else(|| {
                anyhow!(
                    "internal texture capability validation error: pass '{}' samples undeclared texture '{}'",
                    pass_id,
                    texture_name
                )
            })?;
        req.sampled_by_passes.push(pass_id.clone());
    }

    let mut requirements: Vec<TextureCapabilityRequirement> =
        requirements_by_name.into_values().collect();
    requirements.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    for req in &mut requirements {
        req.sampled_by_passes.sort();
        req.sampled_by_passes.dedup();
        req.blend_target_passes.sort();
        req.blend_target_passes.dedup();
    }

    Ok(requirements)
}

pub(crate) fn validate_texture_capability_requirements_with_resolver<F>(
    requirements: &[TextureCapabilityRequirement],
    mut features_for_format: F,
) -> Result<()>
where
    F: FnMut(TextureFormat) -> wgpu::TextureFormatFeatures,
{
    for req in requirements {
        let format_features = features_for_format(req.format);
        let missing_allowed_usages = req.usage - format_features.allowed_usages;
        if !missing_allowed_usages.is_empty() {
            bail!(
                "texture capability validation failed for '{}': format {:?} missing required usages {:?} (allowed {:?})",
                req.name,
                req.format,
                missing_allowed_usages,
                format_features.allowed_usages
            );
        }

        if !format_features
            .flags
            .sample_count_supported(req.sample_count)
        {
            bail!(
                "texture capability validation failed for '{}': format {:?} does not support sample_count={} (supported {:?})",
                req.name,
                req.format,
                req.sample_count,
                format_features.flags.supported_sample_counts()
            );
        }

        if !req.sampled_by_passes.is_empty()
            && !format_features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
        {
            bail!(
                "texture capability validation failed for '{}': format {:?} is sampled by {:?} but FILTERABLE is not supported",
                req.name,
                req.format,
                req.sampled_by_passes
            );
        }

        if !req.blend_target_passes.is_empty()
            && !format_features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
        {
            bail!(
                "texture capability validation failed for '{}': format {:?} is a blend target in {:?} but BLENDABLE is not supported",
                req.name,
                req.format,
                req.blend_target_passes
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_texture_capability_requirements(
    requirements: &[TextureCapabilityRequirement],
    device_features: wgpu::Features,
    adapter: Option<&wgpu::Adapter>,
) -> Result<()> {
    validate_texture_capability_requirements_with_resolver(requirements, |format| {
        effective_texture_format_features(format, device_features, adapter)
    })
}
