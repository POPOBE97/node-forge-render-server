use rust_wgpu_fiber::eframe::wgpu;

const VECTORSCOPE_BINS: u32 = 256;
const VECTORSCOPE_WORD_COUNT: usize = (VECTORSCOPE_BINS * VECTORSCOPE_BINS) as usize;
const VECTORSCOPE_BYTE_COUNT: usize = VECTORSCOPE_WORD_COUNT * std::mem::size_of::<u32>();
const VECTORSCOPE_OUTPUT_SIZE: [u32; 2] = [512, 512];

const COMPUTE_SHADER_SRC: &str = r#"
const BINS: u32 = 256u;

@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> bins: array<atomic<u32>, 65536>;

fn uv_to_bin(cb: f32, cr: f32) -> vec2<u32> {
    let u = clamp(cb + 0.5, 0.0, 1.0);
    let v = clamp(cr + 0.5, 0.0, 1.0);
    let bx = u32(clamp(floor(u * 255.0 + 0.5), 0.0, 255.0));
    let by = u32(clamp(floor(v * 255.0 + 0.5), 0.0, 255.0));
    return vec2<u32>(bx, by);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(source_tex);
    if (gid.x >= size.x || gid.y >= size.y) {
        return;
    }

    let rgba = textureLoad(source_tex, vec2<i32>(gid.xy), 0);
    let rgb = rgba.rgb;

    // BT.709 YCbCr chroma components in approximately [-0.5, 0.5].
    let cb = -0.114572 * rgb.r - 0.385428 * rgb.g + 0.5 * rgb.b;
    let cr = 0.5 * rgb.r - 0.454153 * rgb.g - 0.045847 * rgb.b;

    let uv_bin = uv_to_bin(cb, cr);
    let index = uv_bin.y * BINS + uv_bin.x;
    atomicAdd(&bins[index], 1u);
}
"#;

const RENDER_SHADER_SRC: &str = r#"
struct RenderParams {
    source_width: u32,
    source_height: u32,
    _padding0: u32,
    _padding1: u32,
}

@group(0) @binding(0)
var<storage, read> bins: array<u32, 65536>;

@group(0) @binding(1)
var<uniform> params: RenderParams;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

fn intensity_from_count(count: u32) -> f32 {
    let pixel_count = f32(max(params.source_width * params.source_height, 1u));
    let density = f32(count) * 256.0 / pixel_count;
    return 1.0 - exp(-density * 8.0);
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
    let bx = u32(clamp(floor(in.uv.x * 256.0), 0.0, 255.0));
    let by = u32(clamp(floor((1.0 - in.uv.y) * 256.0), 0.0, 255.0));
    let count = bins[by * 256u + bx];

    let signal = intensity_from_count(count);

    let center = vec2<f32>(0.5, 0.5);
    let d = in.uv - center;
    let radius = length(d);

    let px = 1.0 / 512.0;
    let cross_x = 1.0 - smoothstep(0.0, px * 1.5, abs(d.x));
    let cross_y = 1.0 - smoothstep(0.0, px * 1.5, abs(d.y));

    let ring_w = px * 1.5;
    let ring1 = 1.0 - smoothstep(ring_w, ring_w * 2.0, abs(radius - 0.15));
    let ring2 = 1.0 - smoothstep(ring_w, ring_w * 2.0, abs(radius - 0.30));
    let ring3 = 1.0 - smoothstep(ring_w, ring_w * 2.0, abs(radius - 0.45));

    let grid = max(max(cross_x, cross_y), max(ring1, max(ring2, ring3)));

    let bg = vec3<f32>(0.02, 0.02, 0.02);
    let signal_color = vec3<f32>(0.12, 0.98, 0.78) * signal;
    let grid_color = vec3<f32>(0.16, 0.16, 0.16) * grid;

    return vec4<f32>(bg + signal_color + grid_color, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RenderParams {
    source_width: u32,
    source_height: u32,
    _padding0: u32,
    _padding1: u32,
}

pub struct VectorscopeRenderer {
    compute_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    compute_bind_group_layout: wgpu::BindGroupLayout,
    render_bind_group: wgpu::BindGroup,
    bins_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    clear_bytes: Vec<u8>,
}

impl VectorscopeRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let bins_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.scope.vectorscope.buffer"),
            size: (std::mem::size_of::<u32>() * VECTORSCOPE_WORD_COUNT) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.scope.vectorscope.params"),
            size: std::mem::size_of::<RenderParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.scope.vectorscope.compute"),
            source: wgpu::ShaderSource::Wgsl(COMPUTE_SHADER_SRC.into()),
        });

        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.scope.vectorscope.render"),
            source: wgpu::ShaderSource::Wgsl(RENDER_SHADER_SRC.into()),
        });

        let compute_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.scope.vectorscope.compute.bgl"),
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
                label: Some("sys.scope.vectorscope.compute.layout"),
                bind_group_layouts: &[&compute_bind_group_layout],
                push_constant_ranges: &[],
            });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sys.scope.vectorscope.compute.pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let render_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.scope.vectorscope.render.bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.scope.vectorscope.render.bg"),
            layout: &render_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &bins_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &params_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sys.scope.vectorscope.render.layout"),
                bind_group_layouts: &[&render_bind_group_layout],
                push_constant_ranges: &[],
            });

        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.scope.vectorscope.output"),
            size: wgpu::Extent3d {
                width: VECTORSCOPE_OUTPUT_SIZE[0],
                height: VECTORSCOPE_OUTPUT_SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sys.scope.vectorscope.render.pipeline"),
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
            bins_buffer,
            params_buffer,
            output_texture,
            output_view,
            clear_bytes: vec![0; VECTORSCOPE_BYTE_COUNT],
        }
    }

    pub fn update(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        source_size: [u32; 2],
    ) {
        queue.write_buffer(&self.bins_buffer, 0, &self.clear_bytes);

        let params = RenderParams {
            source_width: source_size[0].max(1),
            source_height: source_size[1].max(1),
            _padding0: 0,
            _padding1: 0,
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        let compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.scope.vectorscope.compute.bg"),
            layout: &self.compute_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.bins_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sys.scope.vectorscope.encoder"),
        });

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.scope.vectorscope.compute.pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.compute_pipeline);
            cpass.set_bind_group(0, &compute_bind_group, &[]);
            let workgroup_x = source_size[0].max(1).div_ceil(16);
            let workgroup_y = source_size[1].max(1).div_ceil(16);
            cpass.dispatch_workgroups(workgroup_x, workgroup_y, 1);
        }

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sys.scope.vectorscope.render.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output_view,
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
        &self.output_view
    }

    pub fn output_size(&self) -> [u32; 2] {
        VECTORSCOPE_OUTPUT_SIZE
    }

    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output_texture
    }
}

#[cfg(test)]
mod tests {
    fn cbcr_bin(rgb: [f32; 3]) -> (u32, u32) {
        let cb = -0.114572 * rgb[0] - 0.385428 * rgb[1] + 0.5 * rgb[2];
        let cr = 0.5 * rgb[0] - 0.454153 * rgb[1] - 0.045847 * rgb[2];
        let u = (cb + 0.5).clamp(0.0, 1.0);
        let v = (cr + 0.5).clamp(0.0, 1.0);
        let x = (u * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u32;
        let y = (v * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u32;
        (x, y)
    }

    #[test]
    fn gray_maps_to_center() {
        let (x, y) = cbcr_bin([0.5, 0.5, 0.5]);
        assert!((x as i32 - 128).abs() <= 1);
        assert!((y as i32 - 128).abs() <= 1);
    }

    #[test]
    fn corners_stay_in_range() {
        let samples = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
        ];

        for rgb in samples {
            let (x, y) = cbcr_bin(rgb);
            assert!(x <= 255);
            assert!(y <= 255);
        }
    }
}
