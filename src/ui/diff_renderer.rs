use rust_wgpu_fiber::eframe::wgpu;

use crate::app::{DiffMetricMode, DiffStats};

const STATS_WORD_COUNT: usize = 5;
const STATS_BYTE_SIZE: u64 = (STATS_WORD_COUNT * std::mem::size_of::<u32>()) as u64;

const DIFF_COMPUTE_SHADER_SRC: &str = r#"
struct DiffParams {
    render_size: vec2<u32>,
    ref_size: vec2<u32>,
    offset_px: vec2<i32>,
    mode: u32,
    _padding: u32,
};

@group(0) @binding(0)
var render_tex: texture_2d<f32>;

@group(0) @binding(1)
var ref_tex: texture_2d<f32>;

@group(0) @binding(2)
var display_out_tex: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(3)
var analysis_out_tex: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(4)
var<uniform> params: DiffParams;

@group(0) @binding(5)
var<storage, read_write> stats: array<atomic<u32>, 5>;

fn metric_diff(render_rgb: vec3<f32>, ref_rgb: vec3<f32>, mode: u32) -> vec3<f32> {
    let delta = render_rgb - ref_rgb;
    let eps = vec3<f32>(1e-5, 1e-5, 1e-5);
    var diff_rgb = vec3<f32>(0.0, 0.0, 0.0);

    if (mode == 0u) {
        diff_rgb = delta * 0.5 + vec3<f32>(0.5, 0.5, 0.5);
    } else if (mode == 1u) {
        diff_rgb = abs(delta);
    } else if (mode == 2u) {
        diff_rgb = delta * delta;
    } else if (mode == 3u) {
        diff_rgb = abs(delta) / max(abs(ref_rgb), eps);
    } else {
        diff_rgb = (delta * delta) / max(ref_rgb * ref_rgb, eps);
    }

    return clamp(diff_rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let in_render = gid.x < params.render_size.x && gid.y < params.render_size.y;
    let in_ref = gid.x < params.ref_size.x && gid.y < params.ref_size.y;
    if (!in_render && !in_ref) {
        return;
    }

    if (in_ref) {
        let dst = vec2<i32>(vec2<u32>(gid.xy));
        let src = dst + params.offset_px;

        var diff_rgb = vec3<f32>(0.0, 0.0, 0.0);
        if (
            src.x >= 0 && src.y >= 0 &&
            src.x < i32(params.render_size.x) &&
            src.y < i32(params.render_size.y)
        ) {
            let render_rgb = textureLoad(render_tex, src, 0).rgb;
            let ref_rgb = textureLoad(ref_tex, dst, 0).rgb;
            diff_rgb = metric_diff(render_rgb, ref_rgb, params.mode);
        }
        textureStore(display_out_tex, dst, vec4<f32>(diff_rgb, 1.0));
    }

    if (in_render) {
        let render_xy = vec2<i32>(vec2<u32>(gid.xy));
        let render_rgb = textureLoad(render_tex, render_xy, 0).rgb;
        let ref_xy = render_xy - params.offset_px;

        var ref_rgb = vec3<f32>(0.0, 0.0, 0.0);
        if (
            ref_xy.x >= 0 && ref_xy.y >= 0 &&
            ref_xy.x < i32(params.ref_size.x) &&
            ref_xy.y < i32(params.ref_size.y)
        ) {
            ref_rgb = textureLoad(ref_tex, ref_xy, 0).rgb;
        }

        let stats_rgb = metric_diff(render_rgb, ref_rgb, params.mode);
        textureStore(analysis_out_tex, render_xy, vec4<f32>(stats_rgb, 1.0));
        let scalar = clamp((stats_rgb.r + stats_rgb.g + stats_rgb.b) / 3.0, 0.0, 1.0);
        let q = u32(round(scalar * 255.0));

        atomicMin(&stats[0], q);
        atomicMax(&stats[1], q);
        atomicAdd(&stats[2], q);
        atomicAdd(&stats[3], 1u);

        let h = ((gid.x * 73856093u) ^ (gid.y * 19349663u) ^ (q * 83492791u));
        atomicXor(&stats[4], h);
    }
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DiffParams {
    render_size: [u32; 2],
    ref_size: [u32; 2],
    offset_px: [i32; 2],
    mode: u32,
    _padding: u32,
}

pub struct DiffRenderer {
    output_texture: wgpu::Texture,
    output_texture_view: wgpu::TextureView,
    output_size: [u32; 2],
    analysis_texture: wgpu::Texture,
    analysis_texture_view: wgpu::TextureView,
    analysis_size: [u32; 2],
    compute_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    stats_buffer: wgpu::Buffer,
    stats_readback_buffer: wgpu::Buffer,
    stats_clear_bytes: [u8; STATS_BYTE_SIZE as usize],
}

impl DiffRenderer {
    fn create_storage_texture(
        device: &wgpu::Device,
        label: &'static str,
        size: [u32; 2],
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
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    pub fn new(device: &wgpu::Device, output_size: [u32; 2]) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.diff.compute"),
            source: wgpu::ShaderSource::Wgsl(DIFF_COMPUTE_SHADER_SRC.into()),
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
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
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
            Self::create_storage_texture(device, "sys.diff.output", output_size);
        let analysis_size = [1, 1];
        let (analysis_texture, analysis_texture_view) =
            Self::create_storage_texture(device, "sys.diff.analysis", analysis_size);

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.diff.params"),
            size: std::mem::size_of::<DiffParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let stats_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.diff.stats"),
            size: STATS_BYTE_SIZE,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let stats_readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sys.diff.stats.readback"),
            size: STATS_BYTE_SIZE,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut stats_clear_words = [0u32; STATS_WORD_COUNT];
        stats_clear_words[0] = u32::MAX;
        stats_clear_words[1] = 0;
        stats_clear_words[2] = 0;
        stats_clear_words[3] = 0;
        stats_clear_words[4] = 0;
        let stats_clear_bytes = bytemuck::cast(stats_clear_words);

        Self {
            output_texture,
            output_texture_view,
            output_size,
            analysis_texture,
            analysis_texture_view,
            analysis_size,
            compute_pipeline,
            bind_group_layout,
            params_buffer,
            stats_buffer,
            stats_readback_buffer,
            stats_clear_bytes,
        }
    }

    pub fn output_size(&self) -> [u32; 2] {
        self.output_size
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
        metric_mode: DiffMetricMode,
        collect_stats: bool,
    ) -> Option<DiffStats> {
        let next_analysis_size = [render_size[0].max(1), render_size[1].max(1)];
        if self.analysis_size != next_analysis_size {
            let (analysis_texture, analysis_texture_view) =
                Self::create_storage_texture(device, "sys.diff.analysis", next_analysis_size);
            self.analysis_texture = analysis_texture;
            self.analysis_texture_view = analysis_texture_view;
            self.analysis_size = next_analysis_size;
        }

        let params = DiffParams {
            render_size,
            ref_size,
            offset_px,
            mode: metric_mode.shader_code(),
            _padding: 0,
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
        queue.write_buffer(&self.stats_buffer, 0, &self.stats_clear_bytes);

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
                        buffer: &self.stats_buffer,
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
            let dispatch_width = render_size[0].max(ref_size[0]).max(1);
            let dispatch_height = render_size[1].max(ref_size[1]).max(1);
            let group_x = dispatch_width.div_ceil(16);
            let group_y = dispatch_height.div_ceil(16);
            cpass.dispatch_workgroups(group_x, group_y, 1);
        }

        if collect_stats {
            encoder.copy_buffer_to_buffer(
                &self.stats_buffer,
                0,
                &self.stats_readback_buffer,
                0,
                STATS_BYTE_SIZE,
            );
        }

        queue.submit(std::iter::once(encoder.finish()));

        if !collect_stats {
            return None;
        }

        let slice = self.stats_readback_buffer.slice(..);
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
            self.stats_readback_buffer.unmap();
            return None;
        }

        let mapped = slice.get_mapped_range();
        let words: &[u32] = bytemuck::cast_slice(&mapped);
        let min_q = words.first().copied().unwrap_or(0).min(255);
        let max_q = words.get(1).copied().unwrap_or(0).min(255);
        let sum_q = words.get(2).copied().unwrap_or(0);
        let count = words.get(3).copied().unwrap_or(0);
        drop(mapped);
        self.stats_readback_buffer.unmap();

        if count == 0 {
            return None;
        }

        Some(DiffStats {
            min: min_q as f32 / 255.0,
            max: max_q as f32 / 255.0,
            avg: (sum_q as f32 / count as f32) / 255.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::app::DiffMetricMode;

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

    fn cpu_metric_diff(render_rgb: [f32; 3], ref_rgb: [f32; 3], mode: DiffMetricMode) -> [f32; 3] {
        let delta = [
            render_rgb[0] - ref_rgb[0],
            render_rgb[1] - ref_rgb[1],
            render_rgb[2] - ref_rgb[2],
        ];
        let eps = 1e-5_f32;
        let out = match mode {
            DiffMetricMode::E => [
                delta[0] * 0.5 + 0.5,
                delta[1] * 0.5 + 0.5,
                delta[2] * 0.5 + 0.5,
            ],
            DiffMetricMode::AE => [delta[0].abs(), delta[1].abs(), delta[2].abs()],
            DiffMetricMode::SE => [
                delta[0] * delta[0],
                delta[1] * delta[1],
                delta[2] * delta[2],
            ],
            DiffMetricMode::RAE => [
                delta[0].abs() / ref_rgb[0].abs().max(eps),
                delta[1].abs() / ref_rgb[1].abs().max(eps),
                delta[2].abs() / ref_rgb[2].abs().max(eps),
            ],
            DiffMetricMode::RSE => [
                (delta[0] * delta[0]) / (ref_rgb[0] * ref_rgb[0]).max(eps),
                (delta[1] * delta[1]) / (ref_rgb[1] * ref_rgb[1]).max(eps),
                (delta[2] * delta[2]) / (ref_rgb[2] * ref_rgb[2]).max(eps),
            ],
        };
        [
            out[0].clamp(0.0, 1.0),
            out[1].clamp(0.0, 1.0),
            out[2].clamp(0.0, 1.0),
        ]
    }

    #[test]
    fn stats_reference_mapping_uses_render_minus_offset() {
        assert_eq!(map_ref_xy([5, 7], [2, 3], [16, 16]), Some([3, 4]));
        assert_eq!(map_ref_xy([1, 1], [4, 0], [16, 16]), None);
    }

    #[test]
    fn stats_fallback_to_zero_reference_when_out_of_bounds() {
        let render_rgb = [0.2, 0.6, 1.0];
        let fallback_ref = [0.0, 0.0, 0.0];
        let diff = cpu_metric_diff(render_rgb, fallback_ref, DiffMetricMode::AE);
        assert_eq!(diff, render_rgb);
    }
}
