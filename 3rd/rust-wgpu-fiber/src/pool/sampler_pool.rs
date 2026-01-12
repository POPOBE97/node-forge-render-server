use crate::ResourceName;
use eframe::wgpu;

#[derive(Clone)]
pub struct SamplerSpec {
    pub name: ResourceName,
    pub desc: wgpu::SamplerDescriptor<'static>,
}
