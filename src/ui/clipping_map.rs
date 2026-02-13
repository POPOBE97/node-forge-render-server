use rust_wgpu_fiber::eframe::wgpu;

const COMPUTE_SHADER_SRC: &str = r#"
struct ClipParams {
    size: vec2<u32>,
    shadow_threshold: f32,
    highlight_threshold: f32,
};

@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var out_tex: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(2)
var<uniform> params: ClipParams;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.size.x || gid.y >= params.size.y) {
        return;
    }

    let rgb = textureLoad(source_tex, vec2<i32>(gid.xy), 0).rgb;
    let max_rgb = max(max(rgb.r, rgb.g), rgb.b);
    let min_rgb = min(min(rgb.r, rgb.g), rgb.b);

    let h = smoothstep(params.highlight_threshold - 0.02, params.highlight_threshold, max_rgb);
    let s = 1.0 - smoothstep(params.shadow_threshold, params.shadow_threshold + 0.02, min_rgb);

    var out_rgb = vec3<f32>(0.0, 0.0, 0.0);
    var out_a = 0.0;

    if (h > 0.001 && s > 0.001) {
        out_rgb = vec3<f32>(1.0, 0.0, 1.0);
        out_a = max(h, s) * 0.85;
    } else if (h > 0.001) {
        out_rgb = vec3<f32>(1.0, 0.35, 0.0);
        out_a = h * 0.78;
    } else if (s > 0.001) {
        out_rgb = vec3<f32>(0.0, 0.45, 1.0);
        out_a = s * 0.78;
    }

    textureStore(out_tex, vec2<i32>(gid.xy), vec4<f32>(out_rgb, out_a));
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ClipParams {
    size: [u32; 2],
    shadow_threshold: f32,
    highlight_threshold: f32,
}

pub struct ClippingMapRenderer {
    compute_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    output_size: [u32; 2],
}

impl ClippingMapRenderer {
    pub fn new(device: &wgpu::Device, output_size: [u32; 2]) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.scope.clipping.compute"),
            source: wgpu::ShaderSource::Wgsl(COMPUTE_SHADER_SRC.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sys.scope.clipping.compute.bgl"),
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
            label: Some("sys.scope.clipping.compute.layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sys.scope.clipping.compute.pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.scope.clipping.params"),
            size: std::mem::size_of::<ClipParams>() as u64,
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
            label: Some("sys.scope.clipping.output"),
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
        if target == self.output_size {
            return;
        }

        let (output_texture, output_view, output_size) =
            Self::create_output_texture(device, target);
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
        shadow_threshold: f32,
        highlight_threshold: f32,
    ) {
        self.ensure_output_size(device, source_size);

        let params = ClipParams {
            size: self.output_size,
            shadow_threshold: shadow_threshold.clamp(0.0, 1.0),
            highlight_threshold: highlight_threshold.clamp(0.0, 1.0),
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.scope.clipping.compute.bg"),
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
            label: Some("sys.scope.clipping.encoder"),
        });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.scope.clipping.compute.pass"),
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

    pub fn output_size(&self) -> [u32; 2] {
        self.output_size
    }

    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output_texture
    }
}

#[cfg(test)]
mod tests {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ClipClass {
        None,
        Shadow,
        Highlight,
        Both,
    }

    fn classify(rgb: [f32; 3], shadow: f32, highlight: f32) -> ClipClass {
        let max_rgb = rgb[0].max(rgb[1]).max(rgb[2]);
        let min_rgb = rgb[0].min(rgb[1]).min(rgb[2]);
        let highlight_hit = max_rgb >= highlight;
        let shadow_hit = min_rgb <= shadow;

        match (highlight_hit, shadow_hit) {
            (true, true) => ClipClass::Both,
            (true, false) => ClipClass::Highlight,
            (false, true) => ClipClass::Shadow,
            (false, false) => ClipClass::None,
        }
    }

    #[test]
    fn classify_highlight_only() {
        assert_eq!(classify([0.99, 0.5, 0.4], 0.02, 0.98), ClipClass::Highlight);
    }

    #[test]
    fn classify_shadow_only() {
        assert_eq!(classify([0.01, 0.15, 0.2], 0.02, 0.98), ClipClass::Shadow);
    }

    #[test]
    fn classify_both() {
        assert_eq!(classify([1.0, 0.0, 0.5], 0.02, 0.98), ClipClass::Both);
    }

    #[test]
    fn classify_none() {
        assert_eq!(classify([0.3, 0.4, 0.5], 0.02, 0.98), ClipClass::None);
    }
}
