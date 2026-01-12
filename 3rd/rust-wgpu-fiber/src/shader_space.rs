use crate::{ResourceName, composition::CompositionBuilder};
use crate::pool::{buffer_pool::BufferSpec, texture_pool::TextureSpec, sampler_pool::SamplerSpec};
use eframe::wgpu;
use wgpu::util::DeviceExt;
use std::collections::HashMap;
use std::sync::Arc;

pub type ShaderSpaceResult<T> = Result<T, ShaderSpaceError>;

#[derive(Debug)]
pub enum ShaderSpaceError {
    BufferNotFound(String),
    TextureNotFound(String),
    Other(String),
}

impl std::fmt::Display for ShaderSpaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShaderSpaceError::BufferNotFound(name) => write!(f, "Buffer not found: {}", name),
            ShaderSpaceError::TextureNotFound(name) => write!(f, "Texture not found: {}", name),
            ShaderSpaceError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for ShaderSpaceError {}

pub struct BufferInfo {
    pub wgpu_buffer: wgpu::Buffer,
}

pub struct TextureInfo {
    pub wgpu_texture: wgpu::Texture,
    pub wgpu_texture_view: Option<wgpu::TextureView>,
}

pub struct SamplerInfo {
    pub wgpu_sampler: wgpu::Sampler,
}

pub struct RenderPassBuilder {
    name: ResourceName,
    shader: Option<wgpu::ShaderModuleDescriptor<'static>>,
    uniform_buffers: Vec<(u32, u32, ResourceName, wgpu::ShaderStages)>,
    attribute_buffers: Vec<(u32, ResourceName, wgpu::VertexStepMode, Vec<wgpu::VertexAttribute>)>,
    textures: Vec<(u32, u32, ResourceName, wgpu::ShaderStages)>,
    samplers: Vec<(u32, u32, ResourceName, wgpu::ShaderStages)>,
    color_attachment: Option<ResourceName>,
    blend_state: Option<wgpu::BlendState>,
    load_op: wgpu::LoadOp<wgpu::Color>,
}

impl RenderPassBuilder {
    pub fn new(name: ResourceName) -> Self {
        RenderPassBuilder {
            name,
            shader: None,
            uniform_buffers: Vec::new(),
            attribute_buffers: Vec::new(),
            textures: Vec::new(),
            samplers: Vec::new(),
            color_attachment: None,
            blend_state: None,
            load_op: wgpu::LoadOp::Load,
        }
    }

    pub fn shader(mut self, desc: wgpu::ShaderModuleDescriptor<'static>) -> Self {
        self.shader = Some(desc);
        self
    }

    pub fn bind_uniform_buffer(
        mut self,
        group: u32,
        binding: u32,
        buffer: ResourceName,
        stages: wgpu::ShaderStages,
    ) -> Self {
        self.uniform_buffers.push((group, binding, buffer, stages));
        self
    }

    pub fn bind_attribute_buffer(
        mut self,
        location: u32,
        buffer: ResourceName,
        step_mode: wgpu::VertexStepMode,
        attributes: Vec<wgpu::VertexAttribute>,
    ) -> Self {
        self.attribute_buffers.push((location, buffer, step_mode, attributes));
        self
    }

    pub fn bind_texture(
        mut self,
        group: u32,
        binding: u32,
        texture: ResourceName,
        stages: wgpu::ShaderStages,
    ) -> Self {
        self.textures.push((group, binding, texture, stages));
        self
    }

    pub fn bind_sampler(
        mut self,
        group: u32,
        binding: u32,
        sampler: ResourceName,
        stages: wgpu::ShaderStages,
    ) -> Self {
        self.samplers.push((group, binding, sampler, stages));
        self
    }

    pub fn bind_color_attachment(mut self, texture: ResourceName) -> Self {
        self.color_attachment = Some(texture);
        self
    }

    pub fn blending(mut self, state: wgpu::BlendState) -> Self {
        self.blend_state = Some(state);
        self
    }

    pub fn load_op(mut self, load_op: wgpu::LoadOp<wgpu::Color>) -> Self {
        self.load_op = load_op;
        self
    }
}

pub struct ShaderSpace {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pub buffers: HashMap<String, BufferInfo>,
    pub textures: HashMap<String, TextureInfo>,
    pub samplers: HashMap<String, SamplerInfo>,
    buffer_specs: Vec<BufferSpec>,
    texture_specs: Vec<TextureSpec>,
    sampler_specs: Vec<SamplerSpec>,
    render_passes: Vec<(ResourceName, Box<dyn FnOnce(RenderPassBuilder) -> RenderPassBuilder + Send>)>,
    composition: Option<Box<dyn FnOnce(CompositionBuilder) -> CompositionBuilder + Send>>,
}

impl ShaderSpace {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        ShaderSpace {
            device,
            queue,
            buffers: HashMap::new(),
            textures: HashMap::new(),
            samplers: HashMap::new(),
            buffer_specs: Vec::new(),
            texture_specs: Vec::new(),
            sampler_specs: Vec::new(),
            render_passes: Vec::new(),
            composition: None,
        }
    }

    pub fn declare_buffers(&mut self, specs: Vec<BufferSpec>) {
        self.buffer_specs.extend(specs);
    }

    pub fn declare_textures(&mut self, specs: Vec<TextureSpec>) {
        self.texture_specs.extend(specs);
    }

    pub fn declare_samplers(&mut self, specs: Vec<SamplerSpec>) {
        self.sampler_specs.extend(specs);
    }

    pub fn render_pass<F>(&mut self, name: ResourceName, builder_fn: F)
    where
        F: FnOnce(RenderPassBuilder) -> RenderPassBuilder + Send + 'static,
    {
        self.render_passes.push((name, Box::new(builder_fn)));
    }

    pub fn composite<F>(&mut self, composer_fn: F)
    where
        F: FnOnce(CompositionBuilder) -> CompositionBuilder + Send + 'static,
    {
        self.composition = Some(Box::new(composer_fn));
    }

    /// Prepare all declared resources and render passes.
    /// This should be called once after declaring all resources.
    /// Calling it multiple times will recreate all resources.
    pub fn prepare(&mut self) {
        // Clear existing resources if this is called again
        self.buffers.clear();
        self.textures.clear();
        self.samplers.clear();

        // Create buffers
        for spec in &self.buffer_specs {
            let (name, buffer) = match spec {
                BufferSpec::Init { name, contents, usage } => {
                    let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&format!("buffer_{}", name.as_str())),
                        contents,
                        usage: *usage,
                    });
                    (name.as_str().to_string(), buffer)
                }
                BufferSpec::Sized { name, size, usage } => {
                    let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(&format!("buffer_{}", name.as_str())),
                        size: *size as u64,
                        usage: *usage,
                        mapped_at_creation: false,
                    });
                    (name.as_str().to_string(), buffer)
                }
            };
            self.buffers.insert(name, BufferInfo { wgpu_buffer: buffer });
        }

        // Create textures
        for spec in &self.texture_specs {
            let (name, texture, view) = match spec {
                TextureSpec::Texture { name, resolution, format, usage } => {
                    let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some(&format!("texture_{}", name.as_str())),
                        size: wgpu::Extent3d {
                            width: resolution[0],
                            height: resolution[1],
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: *format,
                        usage: *usage,
                        view_formats: &[],
                    });
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    (name.as_str().to_string(), texture, Some(view))
                }
                TextureSpec::Image { name, image, usage } => {
                    use image::GenericImageView;
                    let dimensions = image.dimensions();
                    let rgba = image.to_rgba8();
                    
                    let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some(&format!("texture_{}", name.as_str())),
                        size: wgpu::Extent3d {
                            width: dimensions.0,
                            height: dimensions.1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        usage: *usage,
                        view_formats: &[],
                    });

                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &rgba,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(4 * dimensions.0),
                            rows_per_image: Some(dimensions.1),
                        },
                        wgpu::Extent3d {
                            width: dimensions.0,
                            height: dimensions.1,
                            depth_or_array_layers: 1,
                        },
                    );

                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    (name.as_str().to_string(), texture, Some(view))
                }
            };
            self.textures.insert(name, TextureInfo {
                wgpu_texture: texture,
                wgpu_texture_view: view,
            });
        }

        // Create samplers
        for spec in &self.sampler_specs {
            let sampler = self.device.create_sampler(&spec.desc);
            self.samplers.insert(
                spec.name.as_str().to_string(),
                SamplerInfo { wgpu_sampler: sampler },
            );
        }

        // Note: In a real implementation, we would create render pipelines and bind groups here.
        // For this minimal stub, we just create the resources.
    }

    pub fn render(&self) {
        // In a real implementation, this would execute all render passes in composition order.
        // For this minimal stub, we do nothing.
    }

    pub fn write_buffer(&self, name: &str, offset: u64, data: &[u8]) -> ShaderSpaceResult<()> {
        let buffer = self
            .buffers
            .get(name)
            .ok_or_else(|| ShaderSpaceError::BufferNotFound(name.to_string()))?;
        self.queue.write_buffer(&buffer.wgpu_buffer, offset, data);
        Ok(())
    }
}
