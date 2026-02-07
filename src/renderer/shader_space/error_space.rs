use std::sync::Arc;

use anyhow::Result;
use rust_wgpu_fiber::{ResourceName, eframe::wgpu, shader_space::ShaderSpace};

use crate::renderer::types::PassBindings;

use super::assembler;

pub(crate) fn build_error_shader_space(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    resolution: [u32; 2],
) -> Result<(ShaderSpace, [u32; 2], ResourceName, Vec<PassBindings>)> {
    assembler::build_error_shader_space_internal(device, queue, resolution)
}
