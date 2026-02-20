use rust_wgpu_fiber::eframe::wgpu;

const COMPUTE_SHADER_SRC: &str = r#"
struct ClampParams {
    size: vec2<u32>,
};

@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var out_tex: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(2)
var<uniform> params: ClampParams;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.size.x || gid.y >= params.size.y) {
        return;
    }

    let src = textureLoad(source_tex, vec2<i32>(gid.xy), 0);
    let clamped = vec4<f32>(
        clamp(src.rgb, vec3<f32>(0.0), vec3<f32>(1.0)),
        clamp(src.a, 0.0, 1.0)
    );
    textureStore(out_tex, vec2<i32>(gid.xy), clamped);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ClampParams {
    size: [u32; 2],
}

pub struct HdrClampRenderer {
    compute_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    output_size: [u32; 2],
}

impl HdrClampRenderer {
    pub fn new(device: &wgpu::Device, output_size: [u32; 2]) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.preview.hdr_clamp.compute"),
            source: wgpu::ShaderSource::Wgsl(COMPUTE_SHADER_SRC.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sys.preview.hdr_clamp.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sys.preview.hdr_clamp.layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sys.preview.hdr_clamp.pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.preview.hdr_clamp.params"),
            size: std::mem::size_of::<ClampParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (output_texture, output_view, output_size) =
            Self::create_output_texture(device, output_size);

        Self {
            compute_pipeline,
            bind_group_layout,
            params_buffer,
            output_texture,
            output_view,
            output_size,
        }
    }

    fn create_output_texture(
        device: &wgpu::Device,
        output_size: [u32; 2],
    ) -> (wgpu::Texture, wgpu::TextureView, [u32; 2]) {
        let output_size = [output_size[0].max(1), output_size[1].max(1)];
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.preview.hdr_clamp.output"),
            size: wgpu::Extent3d {
                width: output_size[0],
                height: output_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        (output_texture, output_view, output_size)
    }

    fn ensure_output_size(&mut self, device: &wgpu::Device, output_size: [u32; 2]) {
        let target = [output_size[0].max(1), output_size[1].max(1)];
        if self.output_size == target {
            return;
        }
        let (output_texture, output_view, output_size) =
            Self::create_output_texture(device, output_size);
        self.output_texture = output_texture;
        self.output_view = output_view;
        self.output_size = output_size;
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        source_size: [u32; 2],
    ) {
        self.ensure_output_size(device, source_size);

        let params = ClampParams {
            size: self.output_size,
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.preview.hdr_clamp.bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.params_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sys.preview.hdr_clamp.encoder"),
        });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.preview.hdr_clamp.pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.compute_pipeline);
            cpass.set_bind_group(0, &bind_group, &[]);
            let group_x = self.output_size[0].div_ceil(16);
            let group_y = self.output_size[1].div_ceil(16);
            cpass.dispatch_workgroups(group_x, group_y, 1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output_view
    }

    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output_texture
    }

    pub fn output_size(&self) -> [u32; 2] {
        self.output_size
    }
}

#[cfg(test)]
mod tests {
    fn clamp_rgba01(rgba: [f32; 4]) -> [f32; 4] {
        [
            rgba[0].clamp(0.0, 1.0),
            rgba[1].clamp(0.0, 1.0),
            rgba[2].clamp(0.0, 1.0),
            rgba[3].clamp(0.0, 1.0),
        ]
    }

    #[test]
    fn clamp_rgba01_limits_overbright_channels() {
        assert_eq!(clamp_rgba01([2.0, 0.25, 1.5, 1.2]), [1.0, 0.25, 1.0, 1.0]);
    }

    #[test]
    fn clamp_rgba01_limits_negative_channels() {
        assert_eq!(clamp_rgba01([-0.1, 0.4, -3.0, -1.0]), [0.0, 0.4, 0.0, 0.0]);
    }
}
