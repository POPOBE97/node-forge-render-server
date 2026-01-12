use crate::ResourceName;
use eframe::wgpu;
use std::sync::Arc;

#[derive(Clone)]
pub enum BufferSpec {
    Init {
        name: ResourceName,
        contents: Arc<[u8]>,
        usage: wgpu::BufferUsages,
    },
    Sized {
        name: ResourceName,
        size: usize,
        usage: wgpu::BufferUsages,
    },
}

impl BufferSpec {
    pub fn name(&self) -> &ResourceName {
        match self {
            BufferSpec::Init { name, .. } => name,
            BufferSpec::Sized { name, .. } => name,
        }
    }
}
