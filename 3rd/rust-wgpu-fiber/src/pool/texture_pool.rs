use crate::ResourceName;
use eframe::wgpu;
use image::DynamicImage;
use std::sync::Arc;

#[derive(Clone)]
pub enum TextureSpec {
    Texture {
        name: ResourceName,
        resolution: [u32; 2],
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    },
    Image {
        name: ResourceName,
        image: Arc<DynamicImage>,
        usage: wgpu::TextureUsages,
    },
}

impl TextureSpec {
    pub fn name(&self) -> &ResourceName {
        match self {
            TextureSpec::Texture { name, .. } => name,
            TextureSpec::Image { name, .. } => name,
        }
    }
}
