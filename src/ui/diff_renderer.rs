use rust_wgpu_fiber::eframe::wgpu;

use crate::app::{DiffMetricMode, DiffStats, RefImageMode};

const WORKGROUP_SIZE_X: u32 = 16;
const WORKGROUP_SIZE_Y: u32 = 16;
const HIST_BIN_COUNT: usize = 512;
const HIST_UNDERFLOW_BIN: usize = 0;
const HIST_ZERO_BIN: usize = 1;
const HIST_INTERIOR_START_BIN: usize = 2;
const HIST_OVERFLOW_BIN: usize = HIST_BIN_COUNT - 1;
const HIST_INTERIOR_BIN_COUNT: usize = HIST_BIN_COUNT - 3;
const HIST_LOG2_MIN: f32 = -24.0;
const HIST_LOG2_MAX: f32 = 24.0;
const HISTOGRAM_BYTE_SIZE: u64 = (HIST_BIN_COUNT * std::mem::size_of::<u32>()) as u64;

const DIFF_COMPUTE_SHADER_TEMPLATE: &str = r#"
struct DiffParams {
    render_size: vec2<u32>,
    ref_size: vec2<u32>,
    offset_px: vec2<i32>,
    compare_mode: u32,
    metric_mode: u32,
    clamp_output: u32,
    groups_x: u32,
    groups_y: u32,
    overlay_opacity: f32,
    _padding: vec4<u32>,
};

@group(0) @binding(0)
var render_tex: texture_2d<f32>;

@group(0) @binding(1)
var ref_tex: texture_2d<f32>;

@group(0) @binding(2)
var display_out_tex: texture_storage_2d<__STORAGE_FORMAT__, write>;

@group(0) @binding(3)
var analysis_out_tex: texture_storage_2d<__STORAGE_FORMAT__, write>;

@group(0) @binding(4)
var<uniform> params: DiffParams;

@group(0) @binding(5)
var<storage, read_write> partial_stats: array<vec4<f32>>;

@group(0) @binding(6)
var<storage, read_write> partial_counts: array<vec4<u32>>;

@group(0) @binding(7)
var<storage, read_write> histogram: array<atomic<u32>, 512>;

var<workgroup> wg_min: array<f32, 256>;
var<workgroup> wg_max: array<f32, 256>;
var<workgroup> wg_sum: array<f32, 256>;
var<workgroup> wg_sum_sq: array<f32, 256>;
var<workgroup> wg_count: array<u32, 256>;
var<workgroup> wg_non_finite: array<u32, 256>;

fn finite_f32(v: f32) -> bool {
    // NaN fails v == v; +/-inf fails abs(v) <= max_f32.
    return (v == v) && (abs(v) <= 3.4028235e38);
}

fn finite_vec4(v: vec4<f32>) -> bool {
    return finite_f32(v.x) && finite_f32(v.y) && finite_f32(v.z) && finite_f32(v.w);
}

fn metric_diff_rgba(render_rgba: vec4<f32>, ref_rgba: vec4<f32>, mode: u32) -> vec4<f32> {
    let delta = render_rgba - ref_rgba;
    let eps = vec4<f32>(1e-5, 1e-5, 1e-5, 1e-5);
    var diff_rgba = vec4<f32>(0.0, 0.0, 0.0, 0.0);

    if (mode == 0u) {
        diff_rgba = delta;
    } else if (mode == 1u) {
        diff_rgba = abs(delta);
    } else if (mode == 2u) {
        diff_rgba = delta * delta;
    } else if (mode == 3u) {
        diff_rgba = abs(delta) / max(abs(ref_rgba), eps);
    } else {
        diff_rgba = (delta * delta) / max(ref_rgba * ref_rgba, eps);
    }

    return diff_rgba;
}

fn compose_overlay(render_rgba: vec4<f32>, ref_rgba: vec4<f32>, opacity: f32) -> vec4<f32> {
    let mix = clamp(opacity, 0.0, 1.0);
    return ref_rgba * mix + render_rgba * (1.0 - mix);
}

// Uniform scalar used for summary stats/histogram:
// average of RGBA channels (equal weighting).
// Average RGB only — alpha is coverage, not luminance.
fn metric_scalar(metric_rgba: vec4<f32>) -> f32 {
    return (metric_rgba.x + metric_rgba.y + metric_rgba.z) / 3.0;
}

fn histogram_bin(v: f32) -> u32 {
    if (v == 0.0) {
        return 1u;
    }

    let log_v = log2(v);
    if (log_v < -24.0) {
        return 0u;
    }
    if (log_v >= 24.0) {
        return 511u;
    }

    let t = (log_v - (-24.0)) / (24.0 - (-24.0));
    let idx = u32(floor(t * 509.0));
    return 2u + min(idx, 508u);
}

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wid: vec3<u32>
) {
    let lane = lid.y * 16u + lid.x;
    let in_render = gid.x < params.render_size.x && gid.y < params.render_size.y;
    var lane_min = 1e30;
    var lane_max = -1e30;
    var lane_sum = 0.0;
    var lane_sum_sq = 0.0;
    var lane_count = 0u;
    var lane_non_finite = 0u;

    if (in_render) {
        let render_xy = vec2<i32>(vec2<u32>(gid.xy));
        let render_rgba = textureLoad(render_tex, render_xy, 0);
        let ref_xy = render_xy - params.offset_px;

        let has_ref = (
            ref_xy.x >= 0 && ref_xy.y >= 0 &&
            ref_xy.x < i32(params.ref_size.x) &&
            ref_xy.y < i32(params.ref_size.y)
        );

        var display_rgba = render_rgba;
        var analysis_rgba = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        if (has_ref) {
            let ref_rgba = textureLoad(ref_tex, ref_xy, 0);
            if (params.compare_mode == 0u) {
                let overlay_rgba = compose_overlay(render_rgba, ref_rgba, params.overlay_opacity);
                display_rgba = overlay_rgba;
                analysis_rgba = overlay_rgba;
            } else {
                let metric_rgba = metric_diff_rgba(render_rgba, ref_rgba, params.metric_mode);
                display_rgba = metric_rgba;
                analysis_rgba = metric_rgba;
            }
        }

        if (params.compare_mode == 1u) {
            // Diff visualization is always opaque for readability.
            display_rgba.a = 1.0;
        }

        // Alpha is always coverage [0,1] regardless of HDR clamp toggle.
        display_rgba.a = clamp(display_rgba.a, 0.0, 1.0);
        analysis_rgba.a = clamp(analysis_rgba.a, 0.0, 1.0);

        if (params.clamp_output != 0u) {
            display_rgba = clamp(display_rgba, vec4<f32>(0.0), vec4<f32>(1.0));
            analysis_rgba = clamp(analysis_rgba, vec4<f32>(0.0), vec4<f32>(1.0));
        }

        textureStore(display_out_tex, render_xy, display_rgba);
        textureStore(analysis_out_tex, render_xy, analysis_rgba);

        if (params.compare_mode == 1u && has_ref) {
            let s = metric_scalar(analysis_rgba);
            if (finite_vec4(analysis_rgba) && finite_f32(s)) {
                lane_min = s;
                lane_max = s;
                lane_sum = s;
                lane_sum_sq = s * s;
                lane_count = 1u;
                let bin = histogram_bin(abs(s));
                atomicAdd(&histogram[bin], 1u);
            } else {
                lane_non_finite = 1u;
            }
        }
    }

    wg_min[lane] = lane_min;
    wg_max[lane] = lane_max;
    wg_sum[lane] = lane_sum;
    wg_sum_sq[lane] = lane_sum_sq;
    wg_count[lane] = lane_count;
    wg_non_finite[lane] = lane_non_finite;
    workgroupBarrier();

    var stride = 128u;
    loop {
        if (stride == 0u) {
            break;
        }

        if (lane < stride) {
            let rhs = lane + stride;
            wg_min[lane] = min(wg_min[lane], wg_min[rhs]);
            wg_max[lane] = max(wg_max[lane], wg_max[rhs]);
            wg_sum[lane] = wg_sum[lane] + wg_sum[rhs];
            wg_sum_sq[lane] = wg_sum_sq[lane] + wg_sum_sq[rhs];
            wg_count[lane] = wg_count[lane] + wg_count[rhs];
            wg_non_finite[lane] = wg_non_finite[lane] + wg_non_finite[rhs];
        }
        workgroupBarrier();
        stride = stride / 2u;
    }

    if (lane == 0u) {
        let group_idx = wid.y * params.groups_x + wid.x;
        partial_stats[group_idx] = vec4<f32>(
            wg_min[0],
            wg_max[0],
            wg_sum[0],
            wg_sum_sq[0]
        );
        partial_counts[group_idx] = vec4<u32>(
            wg_count[0],
            wg_non_finite[0],
            0u,
            0u
        );
    }
}
"#;

fn diff_storage_texture_format_token(output_format: wgpu::TextureFormat) -> &'static str {
    match output_format {
        wgpu::TextureFormat::Rgba8Unorm => "rgba8unorm",
        wgpu::TextureFormat::Rgba16Float => "rgba16float",
        _ => panic!("unsupported diff output storage texture format: {output_format:?}"),
    }
}

fn diff_compute_shader_source(output_format: wgpu::TextureFormat) -> String {
    DIFF_COMPUTE_SHADER_TEMPLATE.replace(
        "__STORAGE_FORMAT__",
        diff_storage_texture_format_token(output_format),
    )
}

pub fn select_diff_output_format(
    render_format: wgpu::TextureFormat,
    ref_format: wgpu::TextureFormat,
) -> wgpu::TextureFormat {
    let needs_high_precision = |format: wgpu::TextureFormat| {
        matches!(
            format,
            wgpu::TextureFormat::Rgba16Unorm
                | wgpu::TextureFormat::Rgba16Float
                | wgpu::TextureFormat::Rgba32Float
        )
    };

    if needs_high_precision(render_format) || needs_high_precision(ref_format) {
        wgpu::TextureFormat::Rgba16Float
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DiffParams {
    render_size: [u32; 2],
    ref_size: [u32; 2],
    offset_px: [i32; 2],
    compare_mode: u32,
    metric_mode: u32,
    clamp_output: u32,
    groups_x: u32,
    groups_y: u32,
    overlay_opacity: f32,
    _padding: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Debug, Default)]
struct PartialStats {
    min: f32,
    max: f32,
    sum: f32,
    sum_sq: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Debug, Default)]
struct PartialCounts {
    count: u32,
    non_finite_count: u32,
    _pad0: u32,
    _pad1: u32,
}

pub struct DiffRenderer {
    output_texture: wgpu::Texture,
    output_texture_view: wgpu::TextureView,
    output_size: [u32; 2],
    output_format: wgpu::TextureFormat,
    analysis_texture: wgpu::Texture,
    analysis_texture_view: wgpu::TextureView,
    analysis_size: [u32; 2],
    compute_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    partial_stats_buffer: wgpu::Buffer,
    partial_counts_buffer: wgpu::Buffer,
    histogram_buffer: wgpu::Buffer,
    partial_stats_readback_buffer: wgpu::Buffer,
    partial_counts_readback_buffer: wgpu::Buffer,
    histogram_readback_buffer: wgpu::Buffer,
    histogram_clear_bytes: Vec<u8>,
    max_stats_groups: u32,
}

impl DiffRenderer {
    fn compare_mode_code(mode: RefImageMode) -> u32 {
        match mode {
            RefImageMode::Overlay => 0,
            RefImageMode::Diff => 1,
        }
    }

    fn create_storage_texture(
        device: &wgpu::Device,
        label: &'static str,
        size: [u32; 2],
        format: wgpu::TextureFormat,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0].max(1),
                height: size[1].max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    fn partial_stats_byte_size(groups: u32) -> u64 {
        (groups.max(1) as u64) * std::mem::size_of::<PartialStats>() as u64
    }

    fn partial_counts_byte_size(groups: u32) -> u64 {
        (groups.max(1) as u64) * std::mem::size_of::<PartialCounts>() as u64
    }

    fn create_partial_stats_buffer(
        device: &wgpu::Device,
        groups: u32,
        label: &'static str,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: Self::partial_stats_byte_size(groups),
            usage,
            mapped_at_creation: false,
        })
    }

    fn create_partial_counts_buffer(
        device: &wgpu::Device,
        groups: u32,
        label: &'static str,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: Self::partial_counts_byte_size(groups),
            usage,
            mapped_at_creation: false,
        })
    }

    fn ensure_stats_capacity(&mut self, device: &wgpu::Device, groups: u32) {
        let groups = groups.max(1);
        if groups <= self.max_stats_groups {
            return;
        }

        self.partial_stats_buffer = Self::create_partial_stats_buffer(
            device,
            groups,
            "sys.diff.stats.partial",
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        self.partial_counts_buffer = Self::create_partial_counts_buffer(
            device,
            groups,
            "sys.diff.stats.counts",
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        self.partial_stats_readback_buffer = Self::create_partial_stats_buffer(
            device,
            groups,
            "sys.diff.stats.partial.readback",
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        self.partial_counts_readback_buffer = Self::create_partial_counts_buffer(
            device,
            groups,
            "sys.diff.stats.counts.readback",
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        self.max_stats_groups = groups;
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
        for _ in 0..200 {
            let _ = device.poll(wgpu::PollType::Poll);
            if let Ok(result) = rx.try_recv() {
                mapped_ok = result.is_ok();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
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

    fn decode_histogram_bin_center(bin: usize) -> f32 {
        if bin <= HIST_UNDERFLOW_BIN {
            return 2.0_f32.powf(HIST_LOG2_MIN);
        }
        if bin == HIST_ZERO_BIN {
            return 0.0;
        }
        if bin >= HIST_OVERFLOW_BIN {
            return 2.0_f32.powf(HIST_LOG2_MAX);
        }

        let step = (HIST_LOG2_MAX - HIST_LOG2_MIN) / HIST_INTERIOR_BIN_COUNT as f32;
        let idx = (bin - HIST_INTERIOR_START_BIN) as f32;
        let center = HIST_LOG2_MIN + (idx + 0.5) * step;
        2.0_f32.powf(center)
    }

    fn p95_abs_from_histogram(histogram: &[u32], sample_count: u64) -> f32 {
        if sample_count == 0 {
            return 0.0;
        }
        let target = (sample_count as f64 * 0.95).ceil() as u64;
        let mut cumulative = 0u64;
        for (bin, count) in histogram.iter().enumerate() {
            cumulative += *count as u64;
            if cumulative >= target {
                return Self::decode_histogram_bin_center(bin);
            }
        }
        Self::decode_histogram_bin_center(HIST_OVERFLOW_BIN)
    }

    pub fn new(
        device: &wgpu::Device,
        output_size: [u32; 2],
        output_format: wgpu::TextureFormat,
    ) -> Self {
        let shader_source = diff_compute_shader_source(output_format);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.diff.compute"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sys.diff.compute.bgl"),
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: output_format,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: output_format,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sys.diff.compute.layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sys.diff.compute.pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let (output_texture, output_texture_view) =
            Self::create_storage_texture(device, "sys.diff.output", output_size, output_format);
        let analysis_size = [1, 1];
        let (analysis_texture, analysis_texture_view) =
            Self::create_storage_texture(device, "sys.diff.analysis", analysis_size, output_format);

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.diff.params"),
            size: std::mem::size_of::<DiffParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let initial_groups = 1;
        let partial_stats_buffer = Self::create_partial_stats_buffer(
            device,
            initial_groups,
            "sys.diff.stats.partial",
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let partial_counts_buffer = Self::create_partial_counts_buffer(
            device,
            initial_groups,
            "sys.diff.stats.counts",
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let histogram_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.diff.stats.histogram"),
            size: HISTOGRAM_BYTE_SIZE,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let partial_stats_readback_buffer = Self::create_partial_stats_buffer(
            device,
            initial_groups,
            "sys.diff.stats.partial.readback",
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let partial_counts_readback_buffer = Self::create_partial_counts_buffer(
            device,
            initial_groups,
            "sys.diff.stats.counts.readback",
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let histogram_readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.diff.stats.histogram.readback"),
            size: HISTOGRAM_BYTE_SIZE,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            output_texture,
            output_texture_view,
            output_size,
            output_format,
            analysis_texture,
            analysis_texture_view,
            analysis_size,
            compute_pipeline,
            bind_group_layout,
            params_buffer,
            partial_stats_buffer,
            partial_counts_buffer,
            histogram_buffer,
            partial_stats_readback_buffer,
            partial_counts_readback_buffer,
            histogram_readback_buffer,
            histogram_clear_bytes: vec![0_u8; HISTOGRAM_BYTE_SIZE as usize],
            max_stats_groups: initial_groups,
        }
    }

    pub fn output_size(&self) -> [u32; 2] {
        self.output_size
    }

    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output_texture_view
    }

    pub fn analysis_output_size(&self) -> [u32; 2] {
        self.analysis_size
    }

    pub fn analysis_output_view(&self) -> &wgpu::TextureView {
        &self.analysis_texture_view
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        render_view: &wgpu::TextureView,
        render_size: [u32; 2],
        ref_view: &wgpu::TextureView,
        ref_size: [u32; 2],
        offset_px: [i32; 2],
        compare_mode: RefImageMode,
        overlay_opacity: f32,
        metric_mode: DiffMetricMode,
        clamp_output: bool,
        collect_stats: bool,
    ) -> Option<DiffStats> {
        let next_output_size = [render_size[0].max(1), render_size[1].max(1)];
        if self.output_size != next_output_size {
            let (output_texture, output_texture_view) = Self::create_storage_texture(
                device,
                "sys.diff.output",
                next_output_size,
                self.output_format,
            );
            self.output_texture = output_texture;
            self.output_texture_view = output_texture_view;
            self.output_size = next_output_size;
        }

        let next_analysis_size = [render_size[0].max(1), render_size[1].max(1)];
        if self.analysis_size != next_analysis_size {
            let (analysis_texture, analysis_texture_view) = Self::create_storage_texture(
                device,
                "sys.diff.analysis",
                next_analysis_size,
                self.output_format,
            );
            self.analysis_texture = analysis_texture;
            self.analysis_texture_view = analysis_texture_view;
            self.analysis_size = next_analysis_size;
        }

        let dispatch_width = render_size[0].max(1);
        let dispatch_height = render_size[1].max(1);
        let group_x = dispatch_width.div_ceil(WORKGROUP_SIZE_X);
        let group_y = dispatch_height.div_ceil(WORKGROUP_SIZE_Y);
        let group_count = (group_x * group_y).max(1);

        self.ensure_stats_capacity(device, group_count);

        let params = DiffParams {
            render_size,
            ref_size,
            offset_px,
            compare_mode: Self::compare_mode_code(compare_mode),
            metric_mode: metric_mode.shader_code(),
            clamp_output: u32::from(clamp_output),
            groups_x: group_x,
            groups_y: group_y,
            overlay_opacity: overlay_opacity.clamp(0.0, 1.0),
            _padding: [0, 0, 0, 0],
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
        queue.write_buffer(&self.histogram_buffer, 0, &self.histogram_clear_bytes);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.diff.compute.bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(render_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(ref_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.output_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.analysis_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.params_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.partial_stats_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.partial_counts_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.histogram_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sys.diff.encoder"),
        });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sys.diff.compute.pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.compute_pipeline);
            cpass.set_bind_group(0, &bind_group, &[]);
            cpass.dispatch_workgroups(group_x, group_y, 1);
        }

        if collect_stats {
            let partial_stats_bytes = Self::partial_stats_byte_size(group_count);
            let partial_counts_bytes = Self::partial_counts_byte_size(group_count);
            encoder.copy_buffer_to_buffer(
                &self.partial_stats_buffer,
                0,
                &self.partial_stats_readback_buffer,
                0,
                partial_stats_bytes,
            );
            encoder.copy_buffer_to_buffer(
                &self.partial_counts_buffer,
                0,
                &self.partial_counts_readback_buffer,
                0,
                partial_counts_bytes,
            );
            encoder.copy_buffer_to_buffer(
                &self.histogram_buffer,
                0,
                &self.histogram_readback_buffer,
                0,
                HISTOGRAM_BYTE_SIZE,
            );
        }

        queue.submit(std::iter::once(encoder.finish()));

        if !collect_stats {
            return None;
        }

        let partial_stats_bytes = Self::partial_stats_byte_size(group_count);
        let partial_counts_bytes = Self::partial_counts_byte_size(group_count);

        let partial_stats_bytes = Self::map_readback_buffer(
            device,
            &self.partial_stats_readback_buffer,
            partial_stats_bytes,
        )?;
        let partial_counts_bytes = Self::map_readback_buffer(
            device,
            &self.partial_counts_readback_buffer,
            partial_counts_bytes,
        )?;
        let histogram_bytes = Self::map_readback_buffer(
            device,
            &self.histogram_readback_buffer,
            HISTOGRAM_BYTE_SIZE,
        )?;

        let partial_stats: &[PartialStats] = bytemuck::cast_slice(&partial_stats_bytes);
        let partial_counts: &[PartialCounts] = bytemuck::cast_slice(&partial_counts_bytes);
        let histogram: &[u32] = bytemuck::cast_slice(&histogram_bytes);

        let mut min_v = f32::INFINITY;
        let mut max_v = f32::NEG_INFINITY;
        let mut sum_v = 0.0_f64;
        let mut sum_sq_v = 0.0_f64;
        let mut sample_count = 0_u64;
        let mut non_finite_count = 0_u64;

        for (stats, counts) in partial_stats.iter().zip(partial_counts.iter()) {
            non_finite_count += counts.non_finite_count as u64;
            if counts.count == 0 {
                continue;
            }
            sample_count += counts.count as u64;
            min_v = min_v.min(stats.min);
            max_v = max_v.max(stats.max);
            sum_v += stats.sum as f64;
            sum_sq_v += stats.sum_sq as f64;
        }

        if sample_count == 0 && non_finite_count == 0 {
            return None;
        }

        let avg = if sample_count == 0 {
            f32::NAN
        } else {
            (sum_v / sample_count as f64) as f32
        };
        let rms = if sample_count == 0 {
            f32::NAN
        } else {
            (sum_sq_v / sample_count as f64).sqrt() as f32
        };

        Some(DiffStats {
            min: if sample_count == 0 { f32::NAN } else { min_v },
            max: if sample_count == 0 { f32::NAN } else { max_v },
            avg,
            rms,
            p95_abs: Self::p95_abs_from_histogram(histogram, sample_count),
            sample_count,
            non_finite_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DiffRenderer, HIST_BIN_COUNT, HIST_INTERIOR_START_BIN, HIST_OVERFLOW_BIN, HIST_ZERO_BIN,
        select_diff_output_format,
    };
    use crate::app::{DiffMetricMode, RefImageMode};
    use rust_wgpu_fiber::eframe::wgpu;

    fn map_ref_xy(
        render_xy: [i32; 2],
        offset_px: [i32; 2],
        ref_size: [u32; 2],
    ) -> Option<[u32; 2]> {
        let rx = render_xy[0] - offset_px[0];
        let ry = render_xy[1] - offset_px[1];
        if rx < 0 || ry < 0 {
            return None;
        }
        if rx >= ref_size[0] as i32 || ry >= ref_size[1] as i32 {
            return None;
        }
        Some([rx as u32, ry as u32])
    }

    fn cpu_metric_diff_rgba(
        render_rgba: [f32; 4],
        ref_rgba: [f32; 4],
        mode: DiffMetricMode,
    ) -> [f32; 4] {
        let delta = [
            render_rgba[0] - ref_rgba[0],
            render_rgba[1] - ref_rgba[1],
            render_rgba[2] - ref_rgba[2],
            render_rgba[3] - ref_rgba[3],
        ];
        let eps = 1e-5_f32;
        match mode {
            DiffMetricMode::E => delta,
            DiffMetricMode::AE => [
                delta[0].abs(),
                delta[1].abs(),
                delta[2].abs(),
                delta[3].abs(),
            ],
            DiffMetricMode::SE => [
                delta[0] * delta[0],
                delta[1] * delta[1],
                delta[2] * delta[2],
                delta[3] * delta[3],
            ],
            DiffMetricMode::RAE => [
                delta[0].abs() / ref_rgba[0].abs().max(eps),
                delta[1].abs() / ref_rgba[1].abs().max(eps),
                delta[2].abs() / ref_rgba[2].abs().max(eps),
                delta[3].abs() / ref_rgba[3].abs().max(eps),
            ],
            DiffMetricMode::RSE => [
                (delta[0] * delta[0]) / (ref_rgba[0] * ref_rgba[0]).max(eps),
                (delta[1] * delta[1]) / (ref_rgba[1] * ref_rgba[1]).max(eps),
                (delta[2] * delta[2]) / (ref_rgba[2] * ref_rgba[2]).max(eps),
                (delta[3] * delta[3]) / (ref_rgba[3] * ref_rgba[3]).max(eps),
            ],
        }
    }

    fn cpu_metric_scalar_rgba(metric_rgba: [f32; 4]) -> f32 {
        (metric_rgba[0] + metric_rgba[1] + metric_rgba[2] + metric_rgba[3]) * 0.25
    }

    fn cpu_compose_overlay(render_rgba: [f32; 4], ref_rgba: [f32; 4], opacity: f32) -> [f32; 4] {
        let mix = opacity.clamp(0.0, 1.0);
        [
            ref_rgba[0] * mix + render_rgba[0] * (1.0 - mix),
            ref_rgba[1] * mix + render_rgba[1] * (1.0 - mix),
            ref_rgba[2] * mix + render_rgba[2] * (1.0 - mix),
            ref_rgba[3] * mix + render_rgba[3] * (1.0 - mix),
        ]
    }

    fn cpu_display_compare_rgba(
        render_rgba: [f32; 4],
        ref_rgba: Option<[f32; 4]>,
        mode: RefImageMode,
        overlay_opacity: f32,
        metric_mode: DiffMetricMode,
        clamp_output: bool,
    ) -> [f32; 4] {
        let mut out = match (mode, ref_rgba) {
            (_, None) => render_rgba,
            (RefImageMode::Overlay, Some(reference)) => {
                cpu_compose_overlay(render_rgba, reference, overlay_opacity)
            }
            (RefImageMode::Diff, Some(reference)) => {
                cpu_metric_diff_rgba(render_rgba, reference, metric_mode)
            }
        };
        if matches!(mode, RefImageMode::Diff) {
            out[3] = 1.0;
        }
        if clamp_output {
            out = out.map(|v| v.clamp(0.0, 1.0));
        }
        out
    }

    fn cpu_overlap_scalar_for_diff(
        render_rgba: [f32; 4],
        ref_rgba: Option<[f32; 4]>,
        metric_mode: DiffMetricMode,
        clamp_output: bool,
    ) -> Option<f32> {
        let reference = ref_rgba?;
        let mut metric = cpu_metric_diff_rgba(render_rgba, reference, metric_mode);
        if clamp_output {
            metric = metric.map(|v| v.clamp(0.0, 1.0));
        }
        Some(cpu_metric_scalar_rgba(metric))
    }

    #[test]
    fn stats_reference_mapping_uses_render_minus_offset() {
        assert_eq!(map_ref_xy([5, 7], [2, 3], [16, 16]), Some([3, 4]));
        assert_eq!(map_ref_xy([1, 1], [4, 0], [16, 16]), None);
    }

    #[test]
    fn overlay_display_outside_reference_uses_render_pixel() {
        let render_rgba = [0.2, 0.6, 1.0, 0.35];
        let out = cpu_display_compare_rgba(
            render_rgba,
            None,
            RefImageMode::Overlay,
            0.75,
            DiffMetricMode::AE,
            false,
        );
        assert_eq!(out, render_rgba);
    }

    #[test]
    fn diff_display_outside_reference_uses_render_rgb_and_forces_opaque_alpha() {
        let render_rgba = [0.2, 0.6, 1.0, 0.35];
        let out = cpu_display_compare_rgba(
            render_rgba,
            None,
            RefImageMode::Diff,
            1.0,
            DiffMetricMode::AE,
            false,
        );
        assert_eq!(out, [0.2, 0.6, 1.0, 1.0]);
    }

    #[test]
    fn diff_display_inside_reference_uses_metric_rgb_and_forces_opaque_alpha() {
        let render_rgba = [0.75, 0.2, 0.1, 0.9];
        let ref_rgba = [0.25, 0.1, 0.4, 0.25];
        let out = cpu_display_compare_rgba(
            render_rgba,
            Some(ref_rgba),
            RefImageMode::Diff,
            1.0,
            DiffMetricMode::E,
            false,
        );
        assert_eq!(out, [0.5, 0.1, -0.3, 1.0]);
    }

    #[test]
    fn overlap_only_stats_skip_non_overlap_pixels() {
        let render_rgba = [0.2, 0.6, 1.0, 0.8];
        let overlap = cpu_overlap_scalar_for_diff(
            render_rgba,
            Some([0.1, 0.2, 0.3, 0.4]),
            DiffMetricMode::AE,
            false,
        );
        let non_overlap = cpu_overlap_scalar_for_diff(render_rgba, None, DiffMetricMode::AE, false);
        assert!(overlap.is_some());
        assert!(non_overlap.is_none());
    }

    #[test]
    fn signed_e_mode_uses_raw_delta_for_all_rgba_channels() {
        let render_rgba = [0.75, 0.2, 0.1, 0.9];
        let ref_rgba = [0.25, 0.1, 0.4, 0.25];
        let diff = cpu_metric_diff_rgba(render_rgba, ref_rgba, DiffMetricMode::E);
        assert_eq!(diff, [0.5, 0.1, -0.3, 0.65]);
    }

    #[test]
    fn stats_scalar_uses_uniform_rgba_channel_weighting() {
        let metric = [1.0, 3.0, 5.0, 7.0];
        assert_eq!(cpu_metric_scalar_rgba(metric), 4.0);
    }

    #[test]
    fn p95_abs_from_histogram_uses_target_cdf_bin() {
        let mut histogram = [0u32; HIST_BIN_COUNT];
        histogram[HIST_ZERO_BIN] = 90;
        let chosen_bin = HIST_INTERIOR_START_BIN + 10;
        histogram[chosen_bin] = 10;

        let p95 = DiffRenderer::p95_abs_from_histogram(&histogram, 100);
        let expected = DiffRenderer::decode_histogram_bin_center(chosen_bin);
        assert!((p95 - expected).abs() <= f32::EPSILON);
    }

    #[test]
    fn histogram_bin_decode_is_monotonic_across_positive_bins() {
        let mut prev = DiffRenderer::decode_histogram_bin_center(HIST_INTERIOR_START_BIN);
        for bin in (HIST_INTERIOR_START_BIN + 1)..=HIST_OVERFLOW_BIN {
            let current = DiffRenderer::decode_histogram_bin_center(bin);
            assert!(current >= prev, "bin {bin} produced non-monotonic decode");
            prev = current;
        }
    }

    #[test]
    fn select_diff_output_format_promotes_when_render_operand_is_rgba16float() {
        assert_eq!(
            select_diff_output_format(
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureFormat::Rgba8Unorm
            ),
            wgpu::TextureFormat::Rgba16Float
        );
    }

    #[test]
    fn select_diff_output_format_promotes_when_reference_operand_is_rgba16float() {
        assert_eq!(
            select_diff_output_format(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba16Float
            ),
            wgpu::TextureFormat::Rgba16Float
        );
    }

    #[test]
    fn select_diff_output_format_promotes_when_reference_operand_is_rgba16unorm() {
        assert_eq!(
            select_diff_output_format(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba16Unorm
            ),
            wgpu::TextureFormat::Rgba16Float
        );
    }

    #[test]
    fn select_diff_output_format_promotes_when_reference_operand_is_rgba32float() {
        assert_eq!(
            select_diff_output_format(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba32Float
            ),
            wgpu::TextureFormat::Rgba16Float
        );
    }

    #[test]
    fn select_diff_output_format_keeps_rgba8unorm_for_non_hdr_operands() {
        assert_eq!(
            select_diff_output_format(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba8Unorm
            ),
            wgpu::TextureFormat::Rgba8Unorm
        );
    }
}
