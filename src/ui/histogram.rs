use rust_wgpu_fiber::eframe::wgpu;

const HISTOGRAM_BINS: u32 = 256;
const HISTOGRAM_CHANNELS: u32 = 3;
const HISTOGRAM_WIDTH: u32 = HISTOGRAM_BINS * HISTOGRAM_CHANNELS;
const HISTOGRAM_HEIGHT: u32 = 400;
const HISTOGRAM_WORD_COUNT: usize = (HISTOGRAM_BINS * HISTOGRAM_CHANNELS) as usize;
const HISTOGRAM_BYTE_COUNT: usize = HISTOGRAM_WORD_COUNT * std::mem::size_of::<u32>();

const COMPUTE_SHADER_SRC: &str = r#"
@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> histogram: array<atomic<u32>, 768>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(source_tex);
    if (gid.x >= size.x || gid.y >= size.y) {
        return;
    }

    let rgba = textureLoad(source_tex, vec2<i32>(gid.xy), 0);
    let r = u32(clamp(rgba.r * 255.0, 0.0, 255.0));
    let g = u32(clamp(rgba.g * 255.0, 0.0, 255.0));
    let b = u32(clamp(rgba.b * 255.0, 0.0, 255.0));

    atomicAdd(&histogram[r], 1u);
    atomicAdd(&histogram[256u + g], 1u);
    atomicAdd(&histogram[512u + b], 1u);
}
"#;

const RENDER_SHADER_SRC: &str = r#"
@group(0) @binding(0)
var<storage, read> histogram: array<u32, 768>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );

    let pos = positions[vertex_index];

    var out: VsOut;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = pos * 0.5 + vec2<f32>(0.5, 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let x_f = clamp(floor(in.uv.x * 768.0), 0.0, 767.0);
    let x = u32(x_f);
    let channel = x % 3u;
    let bin = x / 3u;

    var max_count: u32 = 1u;
    for (var i: u32 = 0u; i < 768u; i = i + 1u) {
        max_count = max(max_count, histogram[i]);
    }

    let inv_max = 1.0 / f32(max_count);
    let channel_offset = channel * 256u;
    let h = f32(histogram[channel_offset + bin]) * inv_max;

    let y_from_bottom = in.uv.y;
    let on = select(0.0, 1.0, y_from_bottom <= h);

    let bg = vec3<f32>(0.031, 0.031, 0.031);
    let channel_color = select(
        select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), channel == 1u),
        vec3<f32>(1.0, 0.0, 0.0),
        channel == 0u,
    );
    let channels = channel_color * on * 0.58;

    return vec4<f32>(bg + channels, 1.0);
}
"#;

pub struct HistogramRenderer {
    compute_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    compute_bind_group_layout: wgpu::BindGroupLayout,
    render_bind_group: wgpu::BindGroup,
    histogram_buffer: wgpu::Buffer,
    output_texture: wgpu::Texture,
    output_texture_view: wgpu::TextureView,
    clear_bytes: [u8; HISTOGRAM_BYTE_COUNT],
}

impl HistogramRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let histogram_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.buffer"),
            size: (std::mem::size_of::<u32>() * HISTOGRAM_WORD_COUNT) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.histogram.compute"),
            source: wgpu::ShaderSource::Wgsl(COMPUTE_SHADER_SRC.into()),
        });

        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.histogram.render"),
            source: wgpu::ShaderSource::Wgsl(RENDER_SHADER_SRC.into()),
        });

        let compute_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.histogram.compute.bgl"),
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
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sys.histogram.compute.layout"),
                bind_group_layouts: &[&compute_bind_group_layout],
                push_constant_ranges: &[],
            });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sys.histogram.compute.pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let render_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.histogram.render.bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.histogram.render.bg"),
            layout: &render_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &histogram_buffer,
                    offset: 0,
                    size: None,
                }),
            }],
        });

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sys.histogram.render.layout"),
                bind_group_layouts: &[&render_bind_group_layout],
                push_constant_ranges: &[],
            });

        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.histogram.output"),
            size: wgpu::Extent3d {
                width: HISTOGRAM_WIDTH,
                height: HISTOGRAM_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let output_texture_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sys.histogram.render.pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &render_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &render_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            compute_pipeline,
            render_pipeline,
            compute_bind_group_layout,
            render_bind_group,
            histogram_buffer,
            output_texture,
            output_texture_view,
            clear_bytes: [0; HISTOGRAM_BYTE_COUNT],
        }
    }

    pub fn update(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        source_size: [u32; 2],
    ) {
        queue.write_buffer(&self.histogram_buffer, 0, &self.clear_bytes);

        let compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.histogram.compute.bg"),
            layout: &self.compute_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.histogram_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sys.histogram.encoder"),
        });

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.histogram.compute.pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.compute_pipeline);
            cpass.set_bind_group(0, &compute_bind_group, &[]);

            let workgroup_x = source_size[0].div_ceil(16);
            let workgroup_y = source_size[1].div_ceil(16);
            cpass.dispatch_workgroups(workgroup_x, workgroup_y, 1);
        }

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sys.histogram.render.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output_texture_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            rpass.set_pipeline(&self.render_pipeline);
            rpass.set_bind_group(0, &self.render_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output_texture_view
    }

    pub fn output_size(&self) -> [u32; 2] {
        [HISTOGRAM_WIDTH, HISTOGRAM_HEIGHT]
    }

    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output_texture
    }
}
