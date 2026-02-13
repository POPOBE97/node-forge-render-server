use rust_wgpu_fiber::eframe::wgpu;

const X_BINS: u32 = 512;
const Y_BINS: u32 = 256;
const PLANES: u32 = 3;
const PLANE_WORD_COUNT: usize = (X_BINS * Y_BINS) as usize;
const TOTAL_WORD_COUNT: usize = (X_BINS * Y_BINS * PLANES) as usize;
const BUFFER_BYTE_COUNT: usize = TOTAL_WORD_COUNT * std::mem::size_of::<u32>();

const PARADE_OUTPUT_SIZE: [u32; 2] = [768, 400];

const COMPUTE_SHADER_SRC: &str = r#"
const X_BINS: u32 = 512u;
const Y_BINS: u32 = 256u;
const PLANE_SIZE: u32 = X_BINS * Y_BINS;

@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> bins: array<atomic<u32>, 393216>;

fn bin_index(plane: u32, x_bin: u32, y_bin: u32) -> u32 {
    return plane * PLANE_SIZE + x_bin * Y_BINS + y_bin;
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(source_tex);
    if (gid.x >= size.x || gid.y >= size.y) {
        return;
    }

    let rgba = textureLoad(source_tex, vec2<i32>(gid.xy), 0);

    let x_bin = min((gid.x * X_BINS) / max(size.x, 1u), X_BINS - 1u);

    let r = u32(clamp(round(rgba.r * 255.0), 0.0, 255.0));
    let g = u32(clamp(round(rgba.g * 255.0), 0.0, 255.0));
    let b = u32(clamp(round(rgba.b * 255.0), 0.0, 255.0));

    let y_r = 255u - r;
    let y_g = 255u - g;
    let y_b = 255u - b;

    atomicAdd(&bins[bin_index(0u, x_bin, y_r)], 1u);
    atomicAdd(&bins[bin_index(1u, x_bin, y_g)], 1u);
    atomicAdd(&bins[bin_index(2u, x_bin, y_b)], 1u);
}
"#;

const RENDER_SHADER_SRC: &str = r#"
const X_BINS: u32 = 512u;
const Y_BINS: u32 = 256u;
const PLANE_SIZE: u32 = X_BINS * Y_BINS;

struct RenderParams {
    source_width: u32,
    source_height: u32,
};

@group(0) @binding(0)
var<storage, read> bins: array<u32, 393216>;

@group(0) @binding(1)
var<uniform> params: RenderParams;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

fn bin_index(plane: u32, x_bin: u32, y_bin: u32) -> u32 {
    return plane * PLANE_SIZE + x_bin * Y_BINS + y_bin;
}

fn intensity_from_count(count: u32) -> f32 {
    let pixel_count = f32(max(params.source_width * params.source_height, 1u));
    let density = f32(count) * f32(X_BINS) / pixel_count;
    return 1.0 - exp(-density * 4.0);
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
fn fs_parade(in: VsOut) -> @location(0) vec4<f32> {
    let x_bin = min(u32(clamp(floor(in.uv.x * f32(X_BINS)), 0.0, f32(X_BINS - 1u))), X_BINS - 1u);
    let y_bin = min(u32(clamp(floor((1.0 - in.uv.y) * f32(Y_BINS)), 0.0, f32(Y_BINS - 1u))), Y_BINS - 1u);

    let r_i = intensity_from_count(bins[bin_index(0u, x_bin, y_bin)]);
    let g_i = intensity_from_count(bins[bin_index(1u, x_bin, y_bin)]);
    let b_i = intensity_from_count(bins[bin_index(2u, x_bin, y_bin)]);
    let signal = vec3<f32>(r_i, g_i, b_i);

    let bg = vec3<f32>(0.027, 0.027, 0.027);
    let guide_x = 1.0 - smoothstep(0.0, 0.003, abs(fract(in.uv.x * 6.0) - 0.5));
    let guide_y = 1.0 - smoothstep(0.0, 0.003, abs(fract(in.uv.y * 4.0) - 0.5));
    let guides = (guide_x + guide_y) * 0.02;

    return vec4<f32>(bg + signal * 1.05 + vec3<f32>(guides), 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RenderParams {
    source_width: u32,
    source_height: u32,
}

pub struct ParadeRenderer {
    compute_pipeline: wgpu::ComputePipeline,
    parade_render_pipeline: wgpu::RenderPipeline,
    compute_bind_group_layout: wgpu::BindGroupLayout,
    render_bind_group: wgpu::BindGroup,
    bins_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    parade_output_texture: wgpu::Texture,
    parade_output_view: wgpu::TextureView,
    clear_bytes: Vec<u8>,
}

impl ParadeRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let bins_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.scope.parade.buffer"),
            size: (std::mem::size_of::<u32>() * TOTAL_WORD_COUNT) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.scope.parade.params"),
            size: std::mem::size_of::<RenderParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.scope.parade.compute"),
            source: wgpu::ShaderSource::Wgsl(COMPUTE_SHADER_SRC.into()),
        });

        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.scope.parade.render"),
            source: wgpu::ShaderSource::Wgsl(RENDER_SHADER_SRC.into()),
        });

        let compute_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.scope.parade.compute.bgl"),
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
                label: Some("sys.scope.parade.compute.layout"),
                bind_group_layouts: &[&compute_bind_group_layout],
                push_constant_ranges: &[],
            });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sys.scope.parade.compute.pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let render_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.scope.parade.render.bgl"),
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
            label: Some("sys.scope.parade.render.bg"),
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
                label: Some("sys.scope.parade.render.layout"),
                bind_group_layouts: &[&render_bind_group_layout],
                push_constant_ranges: &[],
            });

        let parade_output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.scope.parade.output"),
            size: wgpu::Extent3d {
                width: PARADE_OUTPUT_SIZE[0],
                height: PARADE_OUTPUT_SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let parade_output_view =
            parade_output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let parade_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sys.scope.parade.render.pipeline"),
                layout: Some(&render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &render_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &render_shader,
                    entry_point: Some("fs_parade"),
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
            parade_render_pipeline,
            compute_bind_group_layout,
            render_bind_group,
            bins_buffer,
            params_buffer,
            parade_output_texture,
            parade_output_view,
            clear_bytes: vec![0; BUFFER_BYTE_COUNT],
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
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        let compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.scope.parade.compute.bg"),
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
            label: Some("sys.scope.parade.encoder"),
        });

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.scope.parade.compute.pass"),
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
                label: Some("sys.scope.parade.render.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.parade_output_view,
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
            rpass.set_pipeline(&self.parade_render_pipeline);
            rpass.set_bind_group(0, &self.render_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }

    pub fn parade_output_view(&self) -> &wgpu::TextureView {
        &self.parade_output_view
    }

    pub fn parade_output_size(&self) -> [u32; 2] {
        PARADE_OUTPUT_SIZE
    }

    pub fn parade_output_texture(&self) -> &wgpu::Texture {
        &self.parade_output_texture
    }
}

#[cfg(test)]
mod tests {
    use super::{PLANE_WORD_COUNT, X_BINS, Y_BINS};

    fn x_bin(x: u32, width: u32) -> u32 {
        ((x * X_BINS) / width.max(1)).min(X_BINS - 1)
    }

    fn y_bin(v: f32) -> u32 {
        let q = (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        255 - q.min(255)
    }

    #[test]
    fn x_bin_hits_edges() {
        assert_eq!(x_bin(0, 1920), 0);
        assert_eq!(x_bin(1919, 1920), X_BINS - 1);
    }

    #[test]
    fn y_bin_hits_edges() {
        assert_eq!(y_bin(0.0), Y_BINS - 1);
        assert_eq!(y_bin(1.0), 0);
    }

    #[test]
    fn plane_layout_stays_stable() {
        assert_eq!(PLANE_WORD_COUNT, (X_BINS * Y_BINS) as usize);
    }
}
