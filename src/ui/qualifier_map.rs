use rust_wgpu_fiber::eframe::wgpu;

const COMPUTE_SHADER_SRC: &str = r#"
struct QualifierParams {
    size: vec2<u32>,
    r_min: f32,
    r_max: f32,
    g_min: f32,
    g_max: f32,
    b_min: f32,
    b_max: f32,
};

@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var out_tex: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(2)
var<uniform> params: QualifierParams;

const SOFT_BAND: f32 = 0.005;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.size.x || gid.y >= params.size.y) {
        return;
    }

    let rgb = textureLoad(source_tex, vec2<i32>(gid.xy), 0).rgb;

    // Per-channel membership in [min, max] with a small soft band so the edge
    // doesn't visibly stair-step. Each m_* is 1.0 well inside the range, 0.0
    // well outside, smooth in between.
    let mr_lo = smoothstep(params.r_min - SOFT_BAND, params.r_min + SOFT_BAND, rgb.r);
    let mr_hi = 1.0 - smoothstep(params.r_max - SOFT_BAND, params.r_max + SOFT_BAND, rgb.r);
    let m_r = mr_lo * mr_hi;

    let mg_lo = smoothstep(params.g_min - SOFT_BAND, params.g_min + SOFT_BAND, rgb.g);
    let mg_hi = 1.0 - smoothstep(params.g_max - SOFT_BAND, params.g_max + SOFT_BAND, rgb.g);
    let m_g = mg_lo * mg_hi;

    let mb_lo = smoothstep(params.b_min - SOFT_BAND, params.b_min + SOFT_BAND, rgb.b);
    let mb_hi = 1.0 - smoothstep(params.b_max - SOFT_BAND, params.b_max + SOFT_BAND, rgb.b);
    let m_b = mb_lo * mb_hi;

    let m = m_r * m_g * m_b;

    var out_rgb = vec3<f32>(0.0, 0.0, 0.0);
    var out_a = 0.0;
    if (m > 0.001) {
        // High-visibility lime. Distinct from the Clip overlay's
        // magenta / orange / blue palette.
        out_rgb = vec3<f32>(0.71, 1.0, 0.0);
        out_a = m * 0.85;
    }

    textureStore(out_tex, vec2<i32>(gid.xy), vec4<f32>(out_rgb, out_a));
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QualifierParams {
    size: [u32; 2],
    r_min: f32,
    r_max: f32,
    g_min: f32,
    g_max: f32,
    b_min: f32,
    b_max: f32,
}

pub struct QualifierMapRenderer {
    compute_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    output_size: [u32; 2],
}

impl QualifierMapRenderer {
    pub fn new(device: &wgpu::Device, output_size: [u32; 2]) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.scope.qualifier.compute"),
            source: wgpu::ShaderSource::Wgsl(COMPUTE_SHADER_SRC.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sys.scope.qualifier.compute.bgl"),
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
            label: Some("sys.scope.qualifier.compute.layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sys.scope.qualifier.compute.pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.scope.qualifier.params"),
            size: std::mem::size_of::<QualifierParams>() as u64,
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
            label: Some("sys.scope.qualifier.output"),
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

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        source_size: [u32; 2],
        r_min: f32,
        r_max: f32,
        g_min: f32,
        g_max: f32,
        b_min: f32,
        b_max: f32,
    ) {
        self.ensure_output_size(device, source_size);

        let params = QualifierParams {
            size: self.output_size,
            r_min: r_min.clamp(0.0, 1.0),
            r_max: r_max.clamp(0.0, 1.0),
            g_min: g_min.clamp(0.0, 1.0),
            g_max: g_max.clamp(0.0, 1.0),
            b_min: b_min.clamp(0.0, 1.0),
            b_max: b_max.clamp(0.0, 1.0),
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.scope.qualifier.compute.bg"),
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
            label: Some("sys.scope.qualifier.encoder"),
        });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.scope.qualifier.compute.pass"),
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
    fn in_range(v: f32, lo: f32, hi: f32) -> bool {
        v >= lo && v <= hi
    }

    fn classify(rgb: [f32; 3], r: (f32, f32), g: (f32, f32), b: (f32, f32)) -> bool {
        in_range(rgb[0], r.0, r.1) && in_range(rgb[1], g.0, g.1) && in_range(rgb[2], b.0, b.1)
    }

    #[test]
    fn full_open_range_matches_everything() {
        assert!(classify(
            [0.0, 0.5, 1.0],
            (0.0, 1.0),
            (0.0, 1.0),
            (0.0, 1.0)
        ));
    }

    #[test]
    fn out_of_range_in_one_channel_misses() {
        assert!(!classify(
            [0.9, 0.5, 0.5],
            (0.0, 0.5),
            (0.0, 1.0),
            (0.0, 1.0)
        ));
    }

    #[test]
    fn narrow_band_isolates_pixel() {
        assert!(classify(
            [0.42, 0.31, 0.07],
            (0.40, 0.45),
            (0.30, 0.35),
            (0.05, 0.10)
        ));
        assert!(!classify(
            [0.42, 0.31, 0.20],
            (0.40, 0.45),
            (0.30, 0.35),
            (0.05, 0.10)
        ));
    }
}
