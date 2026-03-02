use rust_wgpu_fiber::eframe::wgpu;

const HISTOGRAM_BINS: u32 = 256;
const HISTOGRAM_CHANNELS: u32 = 3;
const HISTOGRAM_WIDTH: u32 = HISTOGRAM_BINS * HISTOGRAM_CHANNELS;
const HISTOGRAM_HEIGHT: u32 = 400;
const HISTOGRAM_WORD_COUNT: usize = (HISTOGRAM_BINS * HISTOGRAM_CHANNELS) as usize;
const HISTOGRAM_BYTE_COUNT: usize = HISTOGRAM_WORD_COUNT * std::mem::size_of::<u32>();
const STATS_WORD_COUNT: usize = 4;
const STATS_BYTE_COUNT: usize = STATS_WORD_COUNT * std::mem::size_of::<u32>();
const HIST_LOG_ADDITION: f32 = 0.001;
const HIST_LOG_DIFF_EPSILON: f32 = 1e-6;
const NTH_NORMALIZATION_FLOOR: f32 = 0.1;
const NTH_NORMALIZATION_HEADROOM: f32 = 1.3;
const MAX_READBACK_POLL_ATTEMPTS: usize = 200;
const READBACK_POLL_SLEEP_MS: u64 = 1;

const STATS_COMPUTE_SHADER_SRC: &str = r#"
@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> stats: array<atomic<u32>, 4>;

fn float_to_ordered(v: f32) -> u32 {
    let bits = bitcast<u32>(v);
    let sign = bits >> 31u;
    return select(bits ^ 0x80000000u, ~bits, sign == 1u);
}

fn is_finite_f32(v: f32) -> bool {
    return v == v && abs(v) <= 3.4028235e38;
}

fn update_stats(v: f32) {
    if (!is_finite_f32(v)) {
        return;
    }

    let key = float_to_ordered(v);
    atomicMin(&stats[0], key);
    atomicMax(&stats[1], key);
    atomicOr(&stats[3], 1u);
    if (v < 0.0 || v > 1.0) {
        atomicOr(&stats[2], 1u);
    }
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(source_tex);
    if (gid.x >= size.x || gid.y >= size.y) {
        return;
    }

    let rgba = textureLoad(source_tex, vec2<i32>(gid.xy), 0);
    update_stats(rgba.r);
    update_stats(rgba.g);
    update_stats(rgba.b);
}
"#;

const HISTOGRAM_COMPUTE_SHADER_SRC: &str = r#"
struct HistogramParams {
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    min_log: f32,
    diff_log: f32,
    _pad3: f32,
    _pad4: f32,
};

@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> histogram: array<atomic<u32>, 768>;

@group(0) @binding(2)
var<uniform> params: HistogramParams;

fn symmetric_log(v: f32) -> f32 {
    return sign(v) * log(abs(v) + 0.001);
}

fn to_hdr_bin(v: f32) -> u32 {
    let diff = max(params.diff_log, 1e-6);
    let s = symmetric_log(v);
    let t = clamp((s - params.min_log) / diff, 0.0, 1.0);
    return u32(clamp(floor(t * 255.0), 0.0, 255.0));
}

fn to_sdr_bin(v: f32) -> u32 {
    return u32(clamp(v * 255.0, 0.0, 255.0));
}

fn is_finite_f32(v: f32) -> bool {
    return v == v && abs(v) <= 3.4028235e38;
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(source_tex);
    if (gid.x >= size.x || gid.y >= size.y) {
        return;
    }

    let rgba = textureLoad(source_tex, vec2<i32>(gid.xy), 0);
    let is_hdr = params.mode == 1u;

    if (is_hdr) {
        if (is_finite_f32(rgba.r)) {
            atomicAdd(&histogram[to_hdr_bin(rgba.r)], 1u);
        }
        if (is_finite_f32(rgba.g)) {
            atomicAdd(&histogram[256u + to_hdr_bin(rgba.g)], 1u);
        }
        if (is_finite_f32(rgba.b)) {
            atomicAdd(&histogram[512u + to_hdr_bin(rgba.b)], 1u);
        }
        return;
    }

    let r = to_sdr_bin(rgba.r);
    let g = to_sdr_bin(rgba.g);
    let b = to_sdr_bin(rgba.b);
    atomicAdd(&histogram[r], 1u);
    atomicAdd(&histogram[256u + g], 1u);
    atomicAdd(&histogram[512u + b], 1u);
}
"#;

const RENDER_SHADER_SRC: &str = r#"
@group(0) @binding(0)
var<storage, read> histogram: array<u32, 768>;

@group(0) @binding(1)
var<storage, read> normalized_histogram: array<f32, 768>;

struct RenderParams {
    mode: u32,
    zero_bin: u32,
    _pad0: vec2<u32>,
}

@group(0) @binding(2)
var<uniform> params: RenderParams;

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

    let channel_offset = channel * 256u;
    let inv_max = 1.0 / f32(max_count);
    let h_raw = f32(histogram[channel_offset + bin]) * inv_max;
    let h_hdr = normalized_histogram[channel_offset + bin];
    let h = select(h_raw, h_hdr, params.mode == 1u);

    let y_from_bottom = in.uv.y;
    let on = select(0.0, 1.0, y_from_bottom <= h);

    let zero_guide = select(0.0, 0.06, params.mode == 1u && bin == params.zero_bin);
    let bg = vec3<f32>(0.031 + zero_guide, 0.031 + zero_guide, 0.031 + zero_guide);
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
    stats_pipeline: wgpu::ComputePipeline,
    histogram_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    stats_bind_group_layout: wgpu::BindGroupLayout,
    histogram_bind_group_layout: wgpu::BindGroupLayout,
    render_bind_group: wgpu::BindGroup,
    histogram_buffer: wgpu::Buffer,
    histogram_readback_buffer: wgpu::Buffer,
    stats_buffer: wgpu::Buffer,
    stats_readback_buffer: wgpu::Buffer,
    histogram_params_buffer: wgpu::Buffer,
    normalized_histogram_buffer: wgpu::Buffer,
    render_params_buffer: wgpu::Buffer,
    output_texture: wgpu::Texture,
    output_texture_view: wgpu::TextureView,
    clear_bytes: [u8; HISTOGRAM_BYTE_COUNT],
    clear_normalized_bytes: [u8; HISTOGRAM_WORD_COUNT * std::mem::size_of::<f32>()],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HistogramParams {
    mode: u32,
    _pad0: [u32; 3],
    min_log: f32,
    diff_log: f32,
    _pad1: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RenderParams {
    mode: u32,
    zero_bin: u32,
    _pad0: [u32; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HistogramMode {
    Sdr = 0,
    Hdr = 1,
    HdrFallbackMax = 2,
}

fn symmetric_log(v: f32) -> f32 {
    v.signum() * (v.abs() + HIST_LOG_ADDITION).ln()
}

fn symmetric_log_inverse(s: f32) -> f32 {
    s.signum() * (s.abs().exp() - HIST_LOG_ADDITION)
}

fn ordered_key_to_float(key: u32) -> f32 {
    // Inverse of WGSL float_to_ordered mapping.
    let bits = if (key >> 31) != 0 {
        key ^ 0x8000_0000
    } else {
        !key
    };
    f32::from_bits(bits)
}

fn map_value_to_bin(v: f32, min_log: f32, diff_log: f32) -> u32 {
    let diff = diff_log.max(HIST_LOG_DIFF_EPSILON);
    let t = ((symmetric_log(v) - min_log) / diff).clamp(0.0, 1.0);
    (t * 255.0).floor().clamp(0.0, 255.0) as u32
}

fn hdr_normalization_index() -> usize {
    let total = HISTOGRAM_WORD_COUNT;
    let channels_to_skip = 1 + (HISTOGRAM_BINS as usize / 128);
    let offset = channels_to_skip * HISTOGRAM_CHANNELS as usize;
    total.saturating_sub(1 + offset)
}

fn normalize_hdr_histogram(
    histogram: &[u32; HISTOGRAM_WORD_COUNT],
    min_log: f32,
    diff_log: f32,
) -> [f32; HISTOGRAM_WORD_COUNT] {
    let mut tmp = [0.0f32; HISTOGRAM_WORD_COUNT];
    let diff = diff_log.max(HIST_LOG_DIFF_EPSILON);
    let bins = HISTOGRAM_BINS as usize;

    for channel in 0..HISTOGRAM_CHANNELS as usize {
        let channel_offset = channel * bins;
        for bin in 0..bins {
            let left_t = bin as f32 / HISTOGRAM_BINS as f32;
            let right_t = (bin as f32 + 1.0) / HISTOGRAM_BINS as f32;
            let left = symmetric_log_inverse(min_log + left_t * diff);
            let right = symmetric_log_inverse(min_log + right_t * diff);
            let width = (right - left).abs().max(HIST_LOG_DIFF_EPSILON);
            let count = histogram[channel_offset + bin] as f32;
            tmp[channel_offset + bin] = count / width;
        }
    }

    let mut sorted = tmp;
    sorted.sort_by(|a, b| a.total_cmp(b));
    let idx = hdr_normalization_index();
    let norm_base = sorted[idx].max(NTH_NORMALIZATION_FLOOR);
    let norm = 1.0 / (norm_base * NTH_NORMALIZATION_HEADROOM);

    let mut normalized = [0.0f32; HISTOGRAM_WORD_COUNT];
    for (dst, value) in normalized.iter_mut().zip(tmp.iter()) {
        *dst = value * norm;
    }
    normalized
}

impl HistogramRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let histogram_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.buffer"),
            size: (std::mem::size_of::<u32>() * HISTOGRAM_WORD_COUNT) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let histogram_readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.readback"),
            size: HISTOGRAM_BYTE_COUNT as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let stats_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.stats"),
            size: STATS_BYTE_COUNT as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let stats_readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.stats.readback"),
            size: STATS_BYTE_COUNT as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let histogram_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.params"),
            size: std::mem::size_of::<HistogramParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let normalized_histogram_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.normalized"),
            size: (HISTOGRAM_WORD_COUNT * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.render.params"),
            size: std::mem::size_of::<RenderParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let stats_compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.histogram.stats.compute"),
            source: wgpu::ShaderSource::Wgsl(STATS_COMPUTE_SHADER_SRC.into()),
        });

        let histogram_compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.histogram.compute"),
            source: wgpu::ShaderSource::Wgsl(HISTOGRAM_COMPUTE_SHADER_SRC.into()),
        });

        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.histogram.render"),
            source: wgpu::ShaderSource::Wgsl(RENDER_SHADER_SRC.into()),
        });

        let stats_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.histogram.stats.compute.bgl"),
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

        let stats_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sys.histogram.stats.compute.layout"),
                bind_group_layouts: &[&stats_bind_group_layout],
                push_constant_ranges: &[],
            });

        let stats_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sys.histogram.stats.compute.pipeline"),
            layout: Some(&stats_pipeline_layout),
            module: &stats_compute_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let histogram_bind_group_layout =
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

        let histogram_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sys.histogram.compute.layout"),
                bind_group_layouts: &[&histogram_bind_group_layout],
                push_constant_ranges: &[],
            });

        let histogram_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sys.histogram.compute.pipeline"),
            layout: Some(&histogram_pipeline_layout),
            module: &histogram_compute_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let render_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.histogram.render.bgl"),
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
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
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
            label: Some("sys.histogram.render.bg"),
            layout: &render_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &histogram_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &normalized_histogram_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &render_params_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
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

        let output_texture_view =
            output_texture.create_view(&wgpu::TextureViewDescriptor::default());

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
            stats_pipeline,
            histogram_pipeline,
            render_pipeline,
            stats_bind_group_layout,
            histogram_bind_group_layout,
            render_bind_group,
            histogram_buffer,
            histogram_readback_buffer,
            stats_buffer,
            stats_readback_buffer,
            histogram_params_buffer,
            normalized_histogram_buffer,
            render_params_buffer,
            output_texture,
            output_texture_view,
            clear_bytes: [0; HISTOGRAM_BYTE_COUNT],
            clear_normalized_bytes: [0; HISTOGRAM_WORD_COUNT * std::mem::size_of::<f32>()],
        }
    }

    fn map_readback_buffer(
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        size: u64,
    ) -> Option<Vec<u8>> {
        let slice = buffer.slice(0..size);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        let mut mapped_ok = false;
        for _ in 0..MAX_READBACK_POLL_ATTEMPTS {
            let _ = device.poll(wgpu::PollType::Poll);
            if let Ok(result) = rx.try_recv() {
                mapped_ok = result.is_ok();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(READBACK_POLL_SLEEP_MS));
        }

        if !mapped_ok {
            buffer.unmap();
            return None;
        }

        let mapped = slice.get_mapped_range();
        let bytes = mapped.to_vec();
        drop(mapped);
        buffer.unmap();
        Some(bytes)
    }

    pub fn update(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        source_size: [u32; 2],
    ) {
        let stats_init = [u32::MAX, 0, 0, 0];
        queue.write_buffer(&self.stats_buffer, 0, bytemuck::cast_slice(&stats_init));

        let stats_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.histogram.stats.compute.bg"),
            layout: &self.stats_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.stats_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });

        let mut stats_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sys.histogram.stats.encoder"),
        });

        {
            let mut cpass = stats_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.histogram.stats.compute.pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.stats_pipeline);
            cpass.set_bind_group(0, &stats_bind_group, &[]);

            let workgroup_x = source_size[0].div_ceil(16);
            let workgroup_y = source_size[1].div_ceil(16);
            cpass.dispatch_workgroups(workgroup_x, workgroup_y, 1);
        }
        stats_encoder.copy_buffer_to_buffer(
            &self.stats_buffer,
            0,
            &self.stats_readback_buffer,
            0,
            STATS_BYTE_COUNT as u64,
        );
        queue.submit(std::iter::once(stats_encoder.finish()));

        let mut mode = HistogramMode::Sdr;
        let mut min_log = 0.0f32;
        let mut diff_log = 1.0f32;
        let mut zero_bin = 0u32;

        if let Some(stats_bytes) =
            Self::map_readback_buffer(device, &self.stats_readback_buffer, STATS_BYTE_COUNT as u64)
        {
            if let Ok(stats) = bytemuck::try_from_bytes::<[u32; STATS_WORD_COUNT]>(&stats_bytes) {
                let has_finite = stats[3] != 0;
                let has_out_of_range = stats[2] != 0;
                if has_finite && has_out_of_range {
                    let min_value = ordered_key_to_float(stats[0]);
                    let max_value = ordered_key_to_float(stats[1]);
                    let min_l = symmetric_log(min_value);
                    let max_l = symmetric_log(max_value);
                    let diff_l = (max_l - min_l).max(HIST_LOG_DIFF_EPSILON);
                    min_log = min_l;
                    diff_log = diff_l;
                    zero_bin = map_value_to_bin(0.0, min_log, diff_log);
                    mode = HistogramMode::Hdr;
                }
            }
        }

        queue.write_buffer(&self.histogram_buffer, 0, &self.clear_bytes);
        let histogram_params = HistogramParams {
            mode: mode as u32,
            _pad0: [0; 3],
            min_log,
            diff_log,
            _pad1: [0.0; 2],
        };
        queue.write_buffer(
            &self.histogram_params_buffer,
            0,
            bytemuck::bytes_of(&histogram_params),
        );

        let histogram_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.histogram.compute.bg"),
            layout: &self.histogram_bind_group_layout,
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.histogram_params_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });

        let mut histogram_encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sys.histogram.compute.encoder"),
            });

        {
            let mut cpass = histogram_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.histogram.compute.pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.histogram_pipeline);
            cpass.set_bind_group(0, &histogram_bind_group, &[]);

            let workgroup_x = source_size[0].div_ceil(16);
            let workgroup_y = source_size[1].div_ceil(16);
            cpass.dispatch_workgroups(workgroup_x, workgroup_y, 1);
        }
        if mode == HistogramMode::Hdr {
            histogram_encoder.copy_buffer_to_buffer(
                &self.histogram_buffer,
                0,
                &self.histogram_readback_buffer,
                0,
                HISTOGRAM_BYTE_COUNT as u64,
            );
        }
        queue.submit(std::iter::once(histogram_encoder.finish()));

        let mut render_mode = mode;
        if mode == HistogramMode::Hdr {
            let normalized = Self::map_readback_buffer(
                device,
                &self.histogram_readback_buffer,
                HISTOGRAM_BYTE_COUNT as u64,
            )
            .and_then(|bytes| {
                bytemuck::try_from_bytes::<[u32; HISTOGRAM_WORD_COUNT]>(&bytes)
                    .ok()
                    .map(|histogram| normalize_hdr_histogram(histogram, min_log, diff_log))
            });

            if let Some(normalized) = normalized {
                queue.write_buffer(
                    &self.normalized_histogram_buffer,
                    0,
                    bytemuck::cast_slice(normalized.as_slice()),
                );
            } else {
                render_mode = HistogramMode::HdrFallbackMax;
                queue.write_buffer(
                    &self.normalized_histogram_buffer,
                    0,
                    &self.clear_normalized_bytes,
                );
            }
        } else {
            queue.write_buffer(
                &self.normalized_histogram_buffer,
                0,
                &self.clear_normalized_bytes,
            );
        }

        let render_params = RenderParams {
            mode: render_mode as u32,
            zero_bin,
            _pad0: [0; 2],
        };
        queue.write_buffer(
            &self.render_params_buffer,
            0,
            bytemuck::bytes_of(&render_params),
        );

        let mut render_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sys.histogram.render.encoder"),
        });

        {
            let mut rpass = render_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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

        queue.submit(std::iter::once(render_encoder.finish()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetric_log_roundtrip() {
        let values = [-10.0, -1.25, -0.01, 0.0, 0.02, 0.9, 5.0, 42.0];
        for value in values {
            let roundtrip = symmetric_log_inverse(symmetric_log(value));
            assert!((roundtrip - value).abs() < 1e-4);
        }
    }

    #[test]
    fn hdr_bin_mapping_is_monotonic() {
        let min_log = symmetric_log(-4.0);
        let diff_log = (symmetric_log(16.0) - min_log).max(HIST_LOG_DIFF_EPSILON);
        let values = [-4.0, -1.0, -0.1, 0.0, 0.1, 1.0, 4.0, 16.0];
        let mut prev = 0u32;
        for (idx, value) in values.into_iter().enumerate() {
            let bin = map_value_to_bin(value, min_log, diff_log);
            if idx > 0 {
                assert!(bin >= prev);
            }
            prev = bin;
        }
    }

    #[test]
    fn zero_bin_sits_between_negative_and_positive() {
        let min_log = symmetric_log(-8.0);
        let diff_log = (symmetric_log(8.0) - min_log).max(HIST_LOG_DIFF_EPSILON);
        let neg = map_value_to_bin(-0.01, min_log, diff_log);
        let zero = map_value_to_bin(0.0, min_log, diff_log);
        let pos = map_value_to_bin(0.01, min_log, diff_log);
        assert!(neg <= zero);
        assert!(zero <= pos);
    }

    #[test]
    fn nth_largest_index_and_fallback_floor() {
        assert_eq!(hdr_normalization_index(), 758);
        let histogram = [0u32; HISTOGRAM_WORD_COUNT];
        let normalized = normalize_hdr_histogram(&histogram, -1.0, 2.0);
        assert!(normalized.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn ordered_float_key_monotonicity() {
        let values = [-3.5, -0.0, 0.0, 0.5, 1200.0];
        let keys = values
            .iter()
            .map(|value| {
                let bits = value.to_bits();
                if (bits >> 31) != 0 {
                    !bits
                } else {
                    bits ^ 0x8000_0000
                }
            })
            .collect::<Vec<_>>();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted);
        assert_eq!(ordered_key_to_float(keys[2]).to_bits(), 0.0f32.to_bits());
    }
}
