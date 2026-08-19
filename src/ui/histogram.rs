use rust_wgpu_fiber::eframe::wgpu;

const HISTOGRAM_BINS: u32 = 256;
const HISTOGRAM_CHANNELS: u32 = 3;
const HISTOGRAM_WIDTH: u32 = HISTOGRAM_BINS * HISTOGRAM_CHANNELS;
const HISTOGRAM_HEIGHT: u32 = 400;
const HISTOGRAM_WORD_COUNT: usize = (HISTOGRAM_BINS * HISTOGRAM_CHANNELS) as usize;
const HISTOGRAM_BYTE_COUNT: usize = HISTOGRAM_WORD_COUNT * std::mem::size_of::<u32>();
const STATS_WORD_COUNT: usize = 4;
const STATS_BYTE_COUNT: usize = STATS_WORD_COUNT * std::mem::size_of::<u32>();

#[cfg(test)]
const HIST_LOG_ADDITION: f32 = 0.001;
#[cfg(test)]
const HIST_LOG_DIFF_EPSILON: f32 = 1e-6;
#[cfg(test)]
const NTH_NORMALIZATION_FLOOR: f32 = 0.1;
#[cfg(test)]
const NTH_NORMALIZATION_HEADROOM: f32 = 1.3;

const STATS_COMPUTE_SHADER_SRC: &str = r#"
@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> stats: array<atomic<u32>, 4>;

var<workgroup> local_stats: array<atomic<u32>, 4>;

fn float_to_ordered(v: f32) -> u32 {
    let bits = bitcast<u32>(v);
    let sign = bits >> 31u;
    return select(bits ^ 0x80000000u, ~bits, sign == 1u);
}

fn is_finite_f32(v: f32) -> bool {
    return v == v && abs(v) <= 3.4028235e38;
}

fn update_local_stats(v: f32) {
    if (!is_finite_f32(v)) {
        return;
    }

    let key = float_to_ordered(v);
    atomicMin(&local_stats[0], key);
    atomicMax(&local_stats[1], key);
    atomicOr(&local_stats[3], 1u);
    if (v < 0.0 || v > 1.0) {
        atomicOr(&local_stats[2], 1u);
    }
}

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    if (local_index < 4u) {
        atomicStore(&local_stats[local_index], select(0u, 0xffffffffu, local_index == 0u));
    }
    workgroupBarrier();

    let size = textureDimensions(source_tex);
    if (gid.x < size.x && gid.y < size.y) {
        let rgba = textureLoad(source_tex, vec2<i32>(gid.xy), 0);
        update_local_stats(rgba.r);
        update_local_stats(rgba.g);
        update_local_stats(rgba.b);
    }
    workgroupBarrier();

    if (local_index == 0u && atomicLoad(&local_stats[3]) != 0u) {
        atomicMin(&stats[0], atomicLoad(&local_stats[0]));
        atomicMax(&stats[1], atomicLoad(&local_stats[1]));
        atomicOr(&stats[2], atomicLoad(&local_stats[2]));
        atomicOr(&stats[3], 1u);
    }
}
"#;

const PREPARE_COMPUTE_SHADER_SRC: &str = r#"
struct HistogramConfig {
    mode: u32,
    zero_bin: u32,
    min_log: f32,
    diff_log: f32,
}

@group(0) @binding(0)
var<storage, read> stats: array<u32, 4>;

@group(0) @binding(1)
var<storage, read_write> config: HistogramConfig;

fn ordered_to_float(key: u32) -> f32 {
    let bits = select(~key, key ^ 0x80000000u, (key >> 31u) != 0u);
    return bitcast<f32>(bits);
}

fn symmetric_log(v: f32) -> f32 {
    return sign(v) * log(1.0 + abs(v) / 0.001);
}

@compute @workgroup_size(1, 1, 1)
fn main() {
    let is_hdr = stats[3] != 0u && stats[2] != 0u;
    config.mode = select(0u, 1u, is_hdr);

    if (!is_hdr) {
        config.zero_bin = 0u;
        config.min_log = 0.0;
        config.diff_log = 1.0;
        return;
    }

    let min_log = symmetric_log(ordered_to_float(stats[0]));
    let max_log = symmetric_log(ordered_to_float(stats[1]));
    let diff_log = max(max_log - min_log, 1e-6);
    let zero_t = clamp((symmetric_log(0.0) - min_log) / diff_log, 0.0, 1.0);

    config.zero_bin = u32(clamp(floor(zero_t * 255.0), 0.0, 255.0));
    config.min_log = min_log;
    config.diff_log = diff_log;
}
"#;

const HISTOGRAM_COMPUTE_SHADER_SRC: &str = r#"
struct HistogramConfig {
    mode: u32,
    zero_bin: u32,
    min_log: f32,
    diff_log: f32,
}

@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> histogram: array<atomic<u32>, 768>;

@group(0) @binding(2)
var<storage, read> config: HistogramConfig;

var<workgroup> local_histogram: array<atomic<u32>, 768>;

fn symmetric_log(v: f32) -> f32 {
    return sign(v) * log(1.0 + abs(v) / 0.001);
}

fn to_hdr_bin(v: f32) -> u32 {
    let diff = max(config.diff_log, 1e-6);
    let s = symmetric_log(v);
    let t = clamp((s - config.min_log) / diff, 0.0, 1.0);
    return u32(clamp(floor(t * 255.0), 0.0, 255.0));
}

fn to_sdr_bin(v: f32) -> u32 {
    return u32(clamp(v * 255.0, 0.0, 255.0));
}

fn is_finite_f32(v: f32) -> bool {
    return v == v && abs(v) <= 3.4028235e38;
}

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    atomicStore(&local_histogram[local_index], 0u);
    atomicStore(&local_histogram[256u + local_index], 0u);
    atomicStore(&local_histogram[512u + local_index], 0u);
    workgroupBarrier();

    let size = textureDimensions(source_tex);
    if (gid.x < size.x && gid.y < size.y) {
        let rgba = textureLoad(source_tex, vec2<i32>(gid.xy), 0);
        if (config.mode == 1u) {
            if (is_finite_f32(rgba.r)) {
                atomicAdd(&local_histogram[to_hdr_bin(rgba.r)], 1u);
            }
            if (is_finite_f32(rgba.g)) {
                atomicAdd(&local_histogram[256u + to_hdr_bin(rgba.g)], 1u);
            }
            if (is_finite_f32(rgba.b)) {
                atomicAdd(&local_histogram[512u + to_hdr_bin(rgba.b)], 1u);
            }
        } else {
            atomicAdd(&local_histogram[to_sdr_bin(rgba.r)], 1u);
            atomicAdd(&local_histogram[256u + to_sdr_bin(rgba.g)], 1u);
            atomicAdd(&local_histogram[512u + to_sdr_bin(rgba.b)], 1u);
        }
    }
    workgroupBarrier();

    let r_count = atomicLoad(&local_histogram[local_index]);
    let g_count = atomicLoad(&local_histogram[256u + local_index]);
    let b_count = atomicLoad(&local_histogram[512u + local_index]);
    if (r_count != 0u) {
        atomicAdd(&histogram[local_index], r_count);
    }
    if (g_count != 0u) {
        atomicAdd(&histogram[256u + local_index], g_count);
    }
    if (b_count != 0u) {
        atomicAdd(&histogram[512u + local_index], b_count);
    }
}
"#;

const NORMALIZE_COMPUTE_SHADER_SRC: &str = r#"
struct HistogramConfig {
    mode: u32,
    zero_bin: u32,
    min_log: f32,
    diff_log: f32,
}

@group(0) @binding(0)
var<storage, read> histogram: array<u32, 768>;

@group(0) @binding(1)
var<storage, read> config: HistogramConfig;

@group(0) @binding(2)
var<storage, read_write> normalized_histogram: array<f32, 768>;

fn symmetric_log_inverse(s: f32) -> f32 {
    return sign(s) * (exp(abs(s)) - 1.0) * 0.001;
}

fn hdr_density(index: u32) -> f32 {
    let bin = index % 256u;
    let diff = max(config.diff_log, 1e-6);
    let left_t = f32(bin) / 256.0;
    let right_t = f32(bin + 1u) / 256.0;
    let left = symmetric_log_inverse(config.min_log + left_t * diff);
    let right = symmetric_log_inverse(config.min_log + right_t * diff);
    let width = max(abs(right - left), 1e-6);
    return f32(histogram[index]) / width;
}

@compute @workgroup_size(1, 1, 1)
fn main() {
    if (config.mode == 0u) {
        var max_count = 1u;
        for (var i = 0u; i < 768u; i = i + 1u) {
            max_count = max(max_count, histogram[i]);
        }
        let inv_max = 1.0 / f32(max_count);
        for (var i = 0u; i < 768u; i = i + 1u) {
            normalized_histogram[i] = f32(histogram[i]) * inv_max;
        }
        return;
    }

    // sorted[758] is the tenth-largest density across all three channels.
    var top_ten: array<f32, 10>;
    for (var i = 0u; i < 768u; i = i + 1u) {
        let value = hdr_density(i);
        if (value > top_ten[0]) {
            top_ten[0] = value;
            for (var j = 0u; j < 9u; j = j + 1u) {
                if (top_ten[j] > top_ten[j + 1u]) {
                    let swap = top_ten[j];
                    top_ten[j] = top_ten[j + 1u];
                    top_ten[j + 1u] = swap;
                }
            }
        }
    }

    let norm_base = max(top_ten[0], 0.1);
    let norm = 1.0 / (norm_base * 1.3);
    for (var i = 0u; i < 768u; i = i + 1u) {
        normalized_histogram[i] = hdr_density(i) * norm;
    }
}
"#;

const RENDER_SHADER_SRC: &str = r#"
struct HistogramConfig {
    mode: u32,
    zero_bin: u32,
    min_log: f32,
    diff_log: f32,
}

@group(0) @binding(0)
var<storage, read> normalized_histogram: array<f32, 768>;

@group(0) @binding(1)
var<storage, read> config: HistogramConfig;

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
    let channel_offset = channel * 256u;
    let h = normalized_histogram[channel_offset + bin];
    let on = select(0.0, 1.0, in.uv.y <= h);

    let zero_guide = select(0.0, 0.06, config.mode == 1u && bin == config.zero_bin);
    let bg = vec3<f32>(0.031 + zero_guide, 0.031 + zero_guide, 0.031 + zero_guide);
    let channel_color = select(
        select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), channel == 1u),
        vec3<f32>(1.0, 0.0, 0.0),
        channel == 0u,
    );
    return vec4<f32>(bg + channel_color * on * 0.58, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HistogramConfig {
    mode: u32,
    zero_bin: u32,
    min_log: f32,
    diff_log: f32,
}

pub struct HistogramRenderer {
    stats_pipeline: wgpu::ComputePipeline,
    prepare_pipeline: wgpu::ComputePipeline,
    histogram_pipeline: wgpu::ComputePipeline,
    normalize_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    stats_bind_group_layout: wgpu::BindGroupLayout,
    histogram_bind_group_layout: wgpu::BindGroupLayout,
    prepare_bind_group: wgpu::BindGroup,
    normalize_bind_group: wgpu::BindGroup,
    render_bind_group: wgpu::BindGroup,
    histogram_buffer: wgpu::Buffer,
    stats_buffer: wgpu::Buffer,
    config_buffer: wgpu::Buffer,
    output_texture: wgpu::Texture,
    output_texture_view: wgpu::TextureView,
    clear_bytes: [u8; HISTOGRAM_BYTE_COUNT],
}

impl HistogramRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let histogram_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.buffer"),
            size: HISTOGRAM_BYTE_COUNT as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let stats_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.stats"),
            size: STATS_BYTE_COUNT as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let config_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.config"),
            size: std::mem::size_of::<HistogramConfig>() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let normalized_histogram_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.histogram.normalized"),
            size: (HISTOGRAM_WORD_COUNT * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let stats_compute_shader = shader(device, "stats", STATS_COMPUTE_SHADER_SRC);
        let prepare_compute_shader = shader(device, "prepare", PREPARE_COMPUTE_SHADER_SRC);
        let histogram_compute_shader = shader(device, "compute", HISTOGRAM_COMPUTE_SHADER_SRC);
        let normalize_compute_shader = shader(device, "normalize", NORMALIZE_COMPUTE_SHADER_SRC);
        let render_shader = shader(device, "render", RENDER_SHADER_SRC);

        let stats_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.histogram.stats.compute.bgl"),
                entries: &[
                    texture_layout_entry(0, wgpu::ShaderStages::COMPUTE),
                    storage_layout_entry(1, wgpu::ShaderStages::COMPUTE, false),
                ],
            });
        let stats_pipeline = create_compute_pipeline(
            device,
            "sys.histogram.stats.compute",
            &stats_bind_group_layout,
            &stats_compute_shader,
        );

        let prepare_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.histogram.prepare.compute.bgl"),
                entries: &[
                    storage_layout_entry(0, wgpu::ShaderStages::COMPUTE, true),
                    storage_layout_entry(1, wgpu::ShaderStages::COMPUTE, false),
                ],
            });
        let prepare_pipeline = create_compute_pipeline(
            device,
            "sys.histogram.prepare.compute",
            &prepare_bind_group_layout,
            &prepare_compute_shader,
        );
        let prepare_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.histogram.prepare.compute.bg"),
            layout: &prepare_bind_group_layout,
            entries: &[
                buffer_entry(0, &stats_buffer),
                buffer_entry(1, &config_buffer),
            ],
        });

        let histogram_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.histogram.compute.bgl"),
                entries: &[
                    texture_layout_entry(0, wgpu::ShaderStages::COMPUTE),
                    storage_layout_entry(1, wgpu::ShaderStages::COMPUTE, false),
                    storage_layout_entry(2, wgpu::ShaderStages::COMPUTE, true),
                ],
            });
        let histogram_pipeline = create_compute_pipeline(
            device,
            "sys.histogram.compute",
            &histogram_bind_group_layout,
            &histogram_compute_shader,
        );

        let normalize_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.histogram.normalize.compute.bgl"),
                entries: &[
                    storage_layout_entry(0, wgpu::ShaderStages::COMPUTE, true),
                    storage_layout_entry(1, wgpu::ShaderStages::COMPUTE, true),
                    storage_layout_entry(2, wgpu::ShaderStages::COMPUTE, false),
                ],
            });
        let normalize_pipeline = create_compute_pipeline(
            device,
            "sys.histogram.normalize.compute",
            &normalize_bind_group_layout,
            &normalize_compute_shader,
        );
        let normalize_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.histogram.normalize.compute.bg"),
            layout: &normalize_bind_group_layout,
            entries: &[
                buffer_entry(0, &histogram_buffer),
                buffer_entry(1, &config_buffer),
                buffer_entry(2, &normalized_histogram_buffer),
            ],
        });

        let render_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sys.histogram.render.bgl"),
                entries: &[
                    storage_layout_entry(0, wgpu::ShaderStages::FRAGMENT, true),
                    storage_layout_entry(1, wgpu::ShaderStages::FRAGMENT, true),
                ],
            });
        let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.histogram.render.bg"),
            layout: &render_bind_group_layout,
            entries: &[
                buffer_entry(0, &normalized_histogram_buffer),
                buffer_entry(1, &config_buffer),
            ],
        });

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sys.histogram.render.layout"),
                bind_group_layouts: &[Some(&render_bind_group_layout)],
                immediate_size: 0,
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
            multiview_mask: None,
            cache: None,
        });

        Self {
            stats_pipeline,
            prepare_pipeline,
            histogram_pipeline,
            normalize_pipeline,
            render_pipeline,
            stats_bind_group_layout,
            histogram_bind_group_layout,
            prepare_bind_group,
            normalize_bind_group,
            render_bind_group,
            histogram_buffer,
            stats_buffer,
            config_buffer,
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
        let stats_init = [u32::MAX, 0, 0, 0];
        queue.write_buffer(&self.stats_buffer, 0, bytemuck::cast_slice(&stats_init));
        queue.write_buffer(&self.histogram_buffer, 0, &self.clear_bytes);

        let stats_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.histogram.stats.compute.bg"),
            layout: &self.stats_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                buffer_entry(1, &self.stats_buffer),
            ],
        });
        let histogram_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.histogram.compute.bg"),
            layout: &self.histogram_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                buffer_entry(1, &self.histogram_buffer),
                buffer_entry(2, &self.config_buffer),
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sys.histogram.gpu_only.encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.histogram.stats.compute.pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.stats_pipeline);
            pass.set_bind_group(0, &stats_bind_group, &[]);
            pass.dispatch_workgroups(source_size[0].div_ceil(16), source_size[1].div_ceil(16), 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.histogram.prepare.compute.pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.prepare_pipeline);
            pass.set_bind_group(0, &self.prepare_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.histogram.compute.pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.histogram_pipeline);
            pass.set_bind_group(0, &histogram_bind_group, &[]);
            pass.dispatch_workgroups(source_size[0].div_ceil(16), source_size[1].div_ceil(16), 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.histogram.normalize.compute.pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.normalize_pipeline);
            pass.set_bind_group(0, &self.normalize_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, &self.render_bind_group, &[]);
            pass.draw(0..3, 0..1);
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

fn shader(device: &wgpu::Device, label: &str, source: &'static str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

fn create_compute_pipeline(
    device: &wgpu::Device,
    label: &str,
    bind_group_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::ComputePipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        module: shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn texture_layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn buffer_entry<'a>(binding: u32, buffer: &'a wgpu::Buffer) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer,
            offset: 0,
            size: None,
        }),
    }
}

#[cfg(test)]
fn symmetric_log(v: f32) -> f32 {
    v.signum() * (1.0 + v.abs() / HIST_LOG_ADDITION).ln()
}

#[cfg(test)]
fn symmetric_log_inverse(s: f32) -> f32 {
    s.signum() * (s.abs().exp() - 1.0) * HIST_LOG_ADDITION
}

#[cfg(test)]
fn ordered_key_to_float(key: u32) -> f32 {
    let bits = if (key >> 31) != 0 {
        key ^ 0x8000_0000
    } else {
        !key
    };
    f32::from_bits(bits)
}

#[cfg(test)]
fn map_value_to_bin(v: f32, min_log: f32, diff_log: f32) -> u32 {
    let diff = diff_log.max(HIST_LOG_DIFF_EPSILON);
    let t = ((symmetric_log(v) - min_log) / diff).clamp(0.0, 1.0);
    (t * 255.0).floor().clamp(0.0, 255.0) as u32
}

#[cfg(test)]
fn hdr_normalization_index() -> usize {
    let channels_to_skip = 1 + (HISTOGRAM_BINS as usize / 128);
    let offset = channels_to_skip * HISTOGRAM_CHANNELS as usize;
    HISTOGRAM_WORD_COUNT.saturating_sub(1 + offset)
}

#[cfg(test)]
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
            tmp[channel_offset + bin] = histogram[channel_offset + bin] as f32 / width;
        }
    }

    let mut sorted = tmp;
    sorted.sort_by(|a, b| a.total_cmp(b));
    let norm_base = sorted[hdr_normalization_index()].max(NTH_NORMALIZATION_FLOOR);
    let norm = 1.0 / (norm_base * NTH_NORMALIZATION_HEADROOM);

    let mut normalized = [0.0f32; HISTOGRAM_WORD_COUNT];
    for (dst, value) in normalized.iter_mut().zip(tmp.iter()) {
        *dst = value * norm;
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr_test_source(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_size: [u32; 2],
        label: &str,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let source = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: source_size[0],
                height: source_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("histogram.test.hdr_clear.encoder"),
        });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("histogram.test.hdr_clear.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &source_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 2.0,
                            g: 0.5,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        queue.submit(std::iter::once(encoder.finish()));
        (source, source_view)
    }

    #[test]
    fn histogram_shaders_are_valid_wgsl() {
        for (label, source) in [
            ("stats", STATS_COMPUTE_SHADER_SRC),
            ("prepare", PREPARE_COMPUTE_SHADER_SRC),
            ("histogram", HISTOGRAM_COMPUTE_SHADER_SRC),
            ("normalize", NORMALIZE_COMPUTE_SHADER_SRC),
            ("render", RENDER_SHADER_SRC),
        ] {
            naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{label} histogram shader is invalid: {error}"));
        }
    }

    #[test]
    fn histogram_gpu_pipeline_submits_without_validation_errors() {
        let headless = rust_wgpu_fiber::HeadlessRenderer::new(Default::default())
            .expect("headless wgpu device should be available");
        let source_size = [32, 16];
        let (_source, source_view) = hdr_test_source(
            &headless.device,
            &headless.queue,
            source_size,
            "histogram.gpu_test.source",
        );
        let histogram = HistogramRenderer::new(&headless.device);

        histogram.update(&headless.device, &headless.queue, &source_view, source_size);
        headless
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(5)),
            })
            .expect("histogram GPU work should complete");
    }

    #[test]
    #[ignore = "manual full-resolution GPU throughput benchmark"]
    fn histogram_gpu_benchmark_120_frames() {
        let headless = rust_wgpu_fiber::HeadlessRenderer::new(Default::default())
            .expect("headless wgpu device should be available");
        let source_size = [1260, 2800];
        let (_source, source_view) = hdr_test_source(
            &headless.device,
            &headless.queue,
            source_size,
            "histogram.benchmark.source",
        );
        let histogram = HistogramRenderer::new(&headless.device);

        for _ in 0..30 {
            histogram.update(&headless.device, &headless.queue, &source_view, source_size);
        }
        headless
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("histogram warmup should complete");

        let start = std::time::Instant::now();
        for _ in 0..120 {
            histogram.update(&headless.device, &headless.queue, &source_view, source_size);
        }
        headless
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("histogram benchmark should complete");
        let elapsed = start.elapsed();
        eprintln!(
            "histogram adapter={:?} frames=120 elapsed_ms={:.3} mean_ms={:.3} throughput_fps={:.1}",
            headless.adapter.get_info().name,
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000.0 / 120.0,
            120.0 / elapsed.as_secs_f64(),
        );
    }

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
    fn tenth_largest_index_and_fallback_floor_match_gpu_contract() {
        assert_eq!(hdr_normalization_index(), 758);
        let histogram = [0u32; HISTOGRAM_WORD_COUNT];
        let normalized = normalize_hdr_histogram(&histogram, -1.0, 2.0);
        assert!(normalized.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn ordered_float_key_monotonicity() {
        let values: [f32; 5] = [-3.5, -0.0, 0.0, 0.5, 1200.0];
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
