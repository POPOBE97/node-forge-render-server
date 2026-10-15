use std::collections::{HashMap, HashSet};

use anyhow::{Result, ensure};
use bytemuck::Zeroable;
use rust_wgpu_fiber::eframe::{egui, egui_wgpu, wgpu};
use wgpu::util::DeviceExt;

use super::{
    calibration::Calibration,
    grid::line_step,
    io::{Assets, Pixels},
    model::*,
};

pub const SHADER: &str = include_str!("render.wgsl");
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    h0: [f32; 4],
    h1: [f32; 4],
    h2: [f32; 4],
    lens: [f32; 4],
    image: [f32; 4],
    view: [f32; 4],
    layer: [f32; 4],
    options: [f32; 4],
    compare: [f32; 4],
    grid_flags: [f32; 4],
    grid: [f32; 4],
    target: [f32; 4],
}
impl Params {
    fn new(
        doc: &PixelPerfectoDocument,
        view: &Viewport,
        size: [u32; 2],
        logical_size: Point,
        gamma: bool,
    ) -> Self {
        let bounds = view.bounds(logical_size);
        let density = size[0] as f64 / (bounds[2] - bounds[0]);
        let g = &doc.grid;
        let mut p = Self::zeroed();
        p.view = [
            bounds[0] as f32,
            bounds[3] as f32,
            ((bounds[2] - bounds[0]) / size[0] as f64) as f32,
            ((bounds[3] - bounds[1]) / size[1] as f64) as f32,
        ];
        p.options = [
            doc.nearest as u8 as f32,
            0.,
            match doc.comparison.mode {
                CompareMode::Overlay => 0.,
                CompareMode::Difference => 1.,
                CompareMode::Ab => 2.,
                CompareMode::Wipe => 3.,
            },
            doc.comparison.show_b as u8 as f32,
        ];
        p.compare = [
            doc.comparison.wipe,
            doc.comparison.gain,
            doc.comparison.horizontal as u8 as f32,
            gamma as u8 as f32,
        ];
        p.grid_flags = [
            g.visible as u8 as f32,
            g.lines as u8 as f32,
            g.centers as u8 as f32,
            g.outlines as u8 as f32,
        ];
        let ratio = density / view.zoom;
        let mut physical_grid = g.clone();
        physical_grid.min_spacing *= ratio as f32;
        p.grid = [
            g.opacity,
            physical_grid.min_spacing,
            density as f32,
            line_step(&physical_grid, density) as f32,
        ];
        p.target = [size[0] as f32, size[1] as f32, 0., 0.];
        p
    }
    fn for_layer(mut self, l: &ImageLayer, premul: bool, opacity: bool, accumulate: bool) -> Self {
        let c = &l.calibration;
        self.h0 = [c.h[0] as f32, c.h[1] as f32, c.h[2] as f32, 0.];
        self.h1 = [c.h[3] as f32, c.h[4] as f32, c.h[5] as f32, 0.];
        self.h2 = [c.h[6] as f32, c.h[7] as f32, c.h[8] as f32, 0.];
        self.lens = c.lens.map(|v| v as f32);
        self.image = [
            l.size[0] as f32,
            l.size[1] as f32,
            c.center[0] as f32,
            c.center[1] as f32,
        ];
        let offset = l.offset_world();
        self.layer = [
            offset[0] as f32,
            offset[1] as f32,
            Calibration::scale(l.size) as f32,
            if opacity { l.opacity } else { 1. },
        ];
        self.options[1] = premul as u8 as f32;
        self.target[3] = accumulate as u8 as f32;
        self
    }
}
struct Source {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    premul: bool,
}
struct Target {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}
impl Target {
    fn new(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.pixelperfecto.target"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            _texture: texture,
            view,
        }
    }
}
pub struct GpuCanvas {
    layout: wgpu::BindGroupLayout,
    warp: wgpu::RenderPipeline,
    compose: wgpu::RenderPipeline,
    sources: HashMap<String, Source>,
    targets: Option<[Target; 3]>,
    size: [u32; 2],
    texture_id: Option<egui::TextureId>,
    last_key: Option<String>,
}
impl GpuCanvas {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sys.pixelperfecto.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sys.pixelperfecto.bindings"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sys.pixelperfecto.layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry, format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sys.pixelperfecto.pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let warp = pipeline("warp", wgpu::TextureFormat::Rgba32Float);
        let compose = pipeline("compose", wgpu::TextureFormat::Rgba16Float);
        Self {
            layout,
            warp,
            compose,
            sources: HashMap::new(),
            targets: None,
            size: [0, 0],
            texture_id: None,
            last_key: None,
        }
    }
    pub fn release(&mut self, renderer: &mut egui_wgpu::Renderer) {
        if let Some(id) = self.texture_id.take() {
            renderer.free_texture(&id);
        }
        self.sources.clear();
        self.targets = None;
        self.last_key = None;
    }
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &str,
        assets: &Assets,
    ) -> Result<()> {
        if self.sources.contains_key(key) {
            return Ok(());
        }
        let asset = assets
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("Image is still loading"))?;
        let image = &asset.image;
        let size = image.size;
        ensure!(
            size[0] <= device.limits().max_texture_dimension_2d
                && size[1] <= device.limits().max_texture_dimension_2d,
            "Image exceeds this GPU's maximum texture dimension ({})",
            device.limits().max_texture_dimension_2d
        );
        let (format, data, stride, premul) = match &image.pixels {
            Pixels::Srgb8(bytes) => (
                wgpu::TextureFormat::Rgba8UnormSrgb,
                bytes.as_slice(),
                4,
                false,
            ),
            Pixels::Linear(values) => (
                wgpu::TextureFormat::Rgba32Float,
                bytemuck::cast_slice(values),
                16,
                true,
            ),
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.pixelperfecto.source"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * stride),
                rows_per_image: Some(size[1]),
            },
            texture.size(),
        );
        let view = texture.create_view(&Default::default());
        self.sources.insert(
            key.into(),
            Source {
                _texture: texture,
                view,
                premul,
            },
        );
        Ok(())
    }
    fn pass(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        a: &wgpu::TextureView,
        b: &wgpu::TextureView,
        params: Params,
        compose: bool,
    ) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sys.pixelperfecto.params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sys.pixelperfecto.draw"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffer.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sys.pixelperfecto.render"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(if compose { &self.compose } else { &self.warp });
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &PixelPerfectoDocument,
        assets: &Assets,
        view: &Viewport,
        size: [u32; 2],
        logical_size: Point,
        gamma: bool,
    ) -> Result<()> {
        let size = [
            size[0].clamp(1, device.limits().max_texture_dimension_2d),
            size[1].clamp(1, device.limits().max_texture_dimension_2d),
        ];
        let key = format!(
            "{}|{:?}|{:?}|{:?}|{}",
            serde_json::to_string(doc)?,
            size,
            logical_size,
            view,
            gamma
        );
        if self.last_key.as_deref() == Some(&key) {
            return Ok(());
        }
        if self.size != size || self.targets.is_none() {
            self.targets = Some([
                Target::new(device, size, wgpu::TextureFormat::Rgba32Float),
                Target::new(device, size, wgpu::TextureFormat::Rgba32Float),
                Target::new(device, size, wgpu::TextureFormat::Rgba16Float),
            ]);
            self.size = size;
        }
        let layers: Vec<_> = if doc.comparison.mode == CompareMode::Overlay {
            doc.layers.iter().filter(|l| l.visible).collect()
        } else {
            doc.layers
                .iter()
                .filter(|l| Some(l.id) == doc.comparison.a || Some(l.id) == doc.comparison.b)
                .collect()
        };
        let wanted: HashSet<_> = layers.iter().map(|l| l.asset.as_str()).collect();
        self.sources.retain(|k, _| wanted.contains(k.as_str()));
        for layer in &layers {
            self.upload(device, queue, &layer.asset, assets)?;
        }
        let targets = self.targets.as_ref().unwrap();
        let params = Params::new(doc, view, size, logical_size, gamma);
        let mut encoder = device.create_command_encoder(&Default::default());
        for t in &targets[..2] {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sys.pixelperfecto.clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let (mut a, b) = (0, 1);
        if doc.comparison.mode == CompareMode::Overlay {
            for layer in layers {
                let src = &self.sources[&layer.asset];
                let out = 1 - a;
                self.pass(
                    device,
                    &mut encoder,
                    &targets[out].view,
                    &src.view,
                    &targets[a].view,
                    params.for_layer(layer, src.premul, true, true),
                    false,
                );
                a = out;
            }
        } else {
            for (index, id) in [doc.comparison.a, doc.comparison.b].into_iter().enumerate() {
                if let Some(layer) = doc.layers.iter().find(|l| Some(l.id) == id) {
                    let src = &self.sources[&layer.asset];
                    self.pass(
                        device,
                        &mut encoder,
                        &targets[index].view,
                        &src.view,
                        &src.view,
                        params.for_layer(layer, src.premul, false, false),
                        false,
                    );
                }
            }
        }
        self.pass(
            device,
            &mut encoder,
            &targets[2].view,
            &targets[a].view,
            &targets[b].view,
            params,
            true,
        );
        queue.submit([encoder.finish()]);
        self.last_key = Some(key);
        Ok(())
    }
    pub fn show(
        &mut self,
        state: &egui_wgpu::RenderState,
        doc: &PixelPerfectoDocument,
        assets: &Assets,
        view: &Viewport,
        rect: egui::Rect,
        ui: &egui::Ui,
    ) -> Result<()> {
        let ppp = ui.ctx().pixels_per_point();
        let size = [
            (rect.width() * ppp).ceil() as u32,
            (rect.height() * ppp).ceil() as u32,
        ];
        let gamma = !state.target_format.is_srgb()
            && state.target_format != wgpu::TextureFormat::Rgba16Float;
        self.render(
            &state.device,
            &state.queue,
            doc,
            assets,
            view,
            size,
            [rect.width() as f64, rect.height() as f64],
            gamma,
        )?;
        let mut renderer = state.renderer.write();
        let target = &self.targets.as_ref().unwrap()[2];
        let sampler = wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        };
        let id = if let Some(id) = self.texture_id {
            renderer.update_egui_texture_from_wgpu_texture_with_sampler_options(
                &state.device,
                &target.view,
                sampler,
                id,
            );
            id
        } else {
            let id = renderer.register_native_texture_with_sampler_options(
                &state.device,
                &target.view,
                sampler,
            );
            self.texture_id = Some(id);
            id
        };
        drop(renderer);
        ui.painter().image(
            id,
            rect,
            egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
            egui::Color32::WHITE,
        );
        Ok(())
    }
    #[cfg(test)]
    pub fn output(&self) -> &wgpu::Texture {
        &self.targets.as_ref().unwrap()[2]._texture
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_perfecto::io::{self, Asset, DecodedImage};
    use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
    use std::sync::Arc;

    fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<[f32; 4]> {
        let w = texture.width();
        let h = texture.height();
        let stride = (w * 8).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: stride as u64 * h as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).unwrap();
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(20)),
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap();
        let mut pixels = vec![];
        for row in data.chunks_exact(stride as usize) {
            for p in row[..w as usize * 8].chunks_exact(8) {
                pixels.push(std::array::from_fn(|c| {
                    half::f16::from_le_bytes([p[c * 2], p[c * 2 + 1]]).to_f32()
                }));
            }
        }
        drop(data);
        buffer.unmap();
        pixels
    }
    #[test]
    fn shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(SHADER)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(SHADER)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
    #[test]
    fn native_gpu_matches_cpu_warp_comparison_alpha_and_grid() {
        let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default())
            .expect("native adapter for pixel comparison");
        assert_ne!(
            gpu.adapter.get_info().backend,
            wgpu::Backend::Noop,
            "This test needs a real GPU"
        );
        let mut assets = Assets::new();
        let mut doc = PixelPerfectoDocument::default();
        doc.grid.visible = false;
        let image_size = [32, 24];
        for id in 1..=2 {
            let mut data = vec![];
            for y in 0..24 {
                for x in 0..32 {
                    data.extend([
                        ((x * 7 + id * 20) % 256) as u8,
                        ((y * 9 + id * 30) % 256) as u8,
                        150,
                        (if x < 2 { 80 } else { 255 }),
                    ]);
                }
            }
            let key = format!("test-{id}");
            let image = DecodedImage {
                size: image_size,
                bit_depth: 8,
                pixels: Pixels::Srgb8(data),
            };
            assets.insert(
                key.clone(),
                Arc::new(Asset {
                    bytes: Arc::new(vec![]),
                    image: Arc::new(image),
                }),
            );
            let mut calibration = Calibration::initial(image_size);
            calibration.h = [0.23, 0.018, 0.03, -0.01, 0.23, -0.02, 0.003, -0.004, 1.];
            calibration.lens = [0.07, -0.01, 0.002, -0.003];
            doc.layers.push(ImageLayer {
                id,
                asset: key,
                name: format!("{id}"),
                size: image_size,
                bit_depth: 8,
                visible: true,
                locked: false,
                opacity: 0.6,
                offset: if id == 1 { [-1, 0] } else { [0, 0] },
                calibration,
                landmarks: vec![],
            });
        }
        doc.repair_selection();
        doc.viewport = Viewport {
            center: [0.1, -0.15],
            zoom: 7.,
        };
        let size = [96, 72];
        let logical = [96., 72.];
        let mut renderer = GpuCanvas::new(&gpu.device);
        for nearest in [false, true] {
            for mode in CompareMode::ALL {
                doc.nearest = nearest;
                doc.comparison.mode = mode;
                doc.comparison.wipe = 0.43;
                renderer
                    .render(
                        &gpu.device,
                        &gpu.queue,
                        &doc,
                        &assets,
                        &doc.viewport,
                        size,
                        logical,
                        false,
                    )
                    .unwrap();
                let pixels = read(&gpu.device, &gpu.queue, renderer.output());
                for y in 0..size[1] {
                    for x in 0..size[0] {
                        let world = doc
                            .viewport
                            .screen_to_world([x as f64 + 0.5, y as f64 + 0.5], logical);
                        let expected = io::composite(
                            &doc,
                            &assets,
                            world,
                            [
                                (x as f64 + 0.5) / size[0] as f64,
                                (y as f64 + 0.5) / size[1] as f64,
                            ],
                            false,
                        );
                        let actual = pixels[(y * size[0] + x) as usize];
                        for c in 0..4 {
                            assert!(
                                (actual[c] - expected[c]).abs() < 0.0015,
                                "{mode:?} nearest={nearest} ({x},{y}) channel {c}: {} vs {}",
                                actual[c],
                                expected[c]
                            );
                        }
                    }
                }
            }
        }
        // Isolated procedural overlay also agrees across different sampling densities.
        doc.layers.clear();
        doc.comparison.mode = CompareMode::Overlay;
        doc.grid.visible = true;
        doc.grid.outlines = true;
        doc.viewport.zoom = 35.;
        renderer
            .render(
                &gpu.device,
                &gpu.queue,
                &doc,
                &assets,
                &doc.viewport,
                size,
                logical,
                false,
            )
            .unwrap();
        let pixels = read(&gpu.device, &gpu.queue, renderer.output());
        for y in 0..size[1] {
            for x in 0..size[0] {
                let world = doc
                    .viewport
                    .screen_to_world([x as f64 + 0.5, y as f64 + 0.5], logical);
                let expected =
                    crate::pixel_perfecto::grid::sample_grid(world, &doc.grid, doc.viewport.zoom);
                for c in 0..4 {
                    assert!(
                        (pixels[(y * size[0] + x) as usize][c] - expected[c]).abs() < 0.002,
                        "grid mismatch at {x},{y}"
                    );
                }
            }
        }
    }
    #[test]
    fn difference_preserves_small_fp32_changes_before_display_conversion() {
        let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).unwrap();
        let mut assets = Assets::new();
        let mut doc = PixelPerfectoDocument::default();
        doc.grid.visible = false;
        for id in [1, 2] {
            let key = format!("float-{id}");
            let value = 0.3123456_f32 + if id == 2 { 0.000001 } else { 0. };
            let values = [value, value, value, 1.].repeat(16);
            assets.insert(
                key.clone(),
                Arc::new(Asset {
                    bytes: Arc::new(vec![]),
                    image: Arc::new(DecodedImage {
                        size: [4, 4],
                        bit_depth: 16,
                        pixels: Pixels::Linear(values),
                    }),
                }),
            );
            doc.layers.push(ImageLayer {
                id,
                asset: key,
                name: format!("{id}"),
                size: [4, 4],
                bit_depth: 16,
                visible: true,
                locked: false,
                opacity: 1.,
                offset: [0, 0],
                calibration: Calibration {
                    h: [0.5, 0., -1., 0., 0.5, -1., 0., 0., 1.],
                    lens: [0.; 4],
                    center: [2., 2.],
                },
                landmarks: vec![],
            });
        }
        doc.repair_selection();
        doc.comparison.mode = CompareMode::Difference;
        doc.viewport = Viewport {
            center: [2., 2.],
            zoom: 1.,
        };
        let mut renderer = GpuCanvas::new(&gpu.device);
        renderer
            .render(
                &gpu.device,
                &gpu.queue,
                &doc,
                &assets,
                &doc.viewport,
                [4, 4],
                [4., 4.],
                false,
            )
            .unwrap();
        let pixels = read(&gpu.device, &gpu.queue, renderer.output());
        for pixel in pixels {
            assert!(
                pixel[0] > 0.0000008 && pixel[0] < 0.0000012,
                "FP32 difference was lost: {pixel:?}"
            );
        }
    }
    #[test]
    #[ignore = "Explicit 10 x 12-megapixel GPU working-set acceptance check"]
    fn ten_12mp_images_pan_without_redecoding_or_resizing_sources() {
        let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).unwrap();
        let mut assets = Assets::new();
        let mut doc = PixelPerfectoDocument::default();
        doc.grid.visible = false;
        for id in 0..10 {
            let key = format!("load-{id}");
            let mut pixels = vec![70 + id as u8 * 12; 4000 * 3000 * 4];
            for p in pixels.chunks_exact_mut(4) {
                p[3] = 255;
            }
            assets.insert(
                key.clone(),
                Arc::new(Asset {
                    bytes: Arc::new(vec![]),
                    image: Arc::new(DecodedImage {
                        size: [4000, 3000],
                        bit_depth: 8,
                        pixels: Pixels::Srgb8(pixels),
                    }),
                }),
            );
            doc.layers.push(ImageLayer {
                id,
                asset: key,
                name: format!("{id}"),
                size: [4000, 3000],
                bit_depth: 8,
                visible: true,
                locked: false,
                opacity: 0.3,
                offset: [0, 0],
                calibration: Calibration::initial([4000, 3000]),
                landmarks: vec![],
            });
        }
        doc.repair_selection();
        doc.viewport.zoom = 12.;
        let mut renderer = GpuCanvas::new(&gpu.device);
        renderer
            .render(
                &gpu.device,
                &gpu.queue,
                &doc,
                &assets,
                &doc.viewport,
                [1600, 1000],
                [1600., 1000.],
                false,
            )
            .unwrap();
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        let start = std::time::Instant::now();
        for frame in 1..=10 {
            doc.viewport.center[0] = frame as f64 * 0.13;
            renderer
                .render(
                    &gpu.device,
                    &gpu.queue,
                    &doc,
                    &assets,
                    &doc.viewport,
                    [1600, 1000],
                    [1600., 1000.],
                    false,
                )
                .unwrap();
        }
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        assert_eq!(renderer.sources.len(), 10);
        for source in renderer.sources.values() {
            assert_eq!(source._texture.width(), 4000);
            assert_eq!(source._texture.height(), 3000);
        }
        eprintln!(
            "Pixel Perfecto: ten 12MP layers at 1600x1000, 10 pan frames: {:?}",
            start.elapsed()
        );
    }
}
