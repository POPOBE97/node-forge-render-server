use crate::native_output::{self, NativeOutput};
use crate::presentation::{self, Output};
use anyhow::{Result, bail};
use node_forge_render_server::color::WorkingColorSpace;
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use rust_wgpu_fiber::wgpu::{self, util::DeviceExt};
use std::{
    ptr::NonNull,
    sync::{Arc, Mutex},
};

pub struct Gpu {
    pub surface: wgpu::Surface<'static>,
    pub adapter: wgpu::Adapter,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub config: wgpu::SurfaceConfiguration,
    pub native_pixels: bool,
    output: Output,
    native: Option<NativeOutput>,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    probe: wgpu::Buffer,
    empty: wgpu::TextureView,
    errors: Arc<Mutex<Vec<String>>>,
    _instance: wgpu::Instance,
    _window: NativeWindow,
}

impl Gpu {
    /// Takes ownership of one acquired ANativeWindow reference.
    pub unsafe fn new(
        window: NonNull<ndk_sys::ANativeWindow>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let result = pollster::block_on(Self::create(window, width, height));
        if result.is_err() {
            unsafe {
                ndk_sys::ANativeWindow_release(window.as_ptr());
            }
        }
        result
    }

    async fn create(
        window: NonNull<ndk_sys::ANativeWindow>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(desc);
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::Android(AndroidDisplayHandle::new())),
                raw_window_handle: RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(
                    window.cast(),
                )),
            })
        }?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await?;
        let caps = surface.get_capabilities(&adapter);
        log::info!(
            "GPU {:?}; surface {:?}",
            adapter.get_info(),
            caps.format_capabilities
        );
        let output = Output::select(&caps, wgpu::TextureFormat::Rgba8Unorm, WorkingColorSpace::Srgb)?;
        let optional = wgpu::Features::FLOAT32_FILTERABLE
            | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
        let (device, queue) = native_output::create_device(&adapter, &wgpu::DeviceDescriptor {
                label: Some("Node Forge Android"),
                required_features: adapter.features() & optional,
                required_limits: adapter.limits(),
                ..Default::default()
            })?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        let pending_errors = errors.clone();
        device.on_uncaptured_error(Arc::new(move |error| {
            log::error!("wgpu: {error}");
            let mut errors = pending_errors.lock().unwrap();
            if errors.len() < 16 {
                errors.push(error.to_string());
            }
        }));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: output.format,
            color_space: output.color_space,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
                wgpu::CompositeAlphaMode::Opaque
            } else {
                caps.alpha_modes[0]
            },
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let pipeline = presentation::create_pipeline(&device, config.format);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let probe = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Presentation mode"),
            contents: bytemuck::cast_slice(&[0.0_f32; 8]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let empty_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Empty scene"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let empty = empty_texture.create_view(&Default::default());
        Ok(Self {
            surface,
            adapter,
            device: Arc::new(device),
            queue: Arc::new(queue),
            config,
            native_pixels: false,
            output,
            native: None,
            pipeline,
            sampler,
            probe,
            empty,
            errors,
            _instance: instance,
            _window: NativeWindow(window),
        })
    }

    pub fn check_errors(&self) -> Result<()> {
        let errors = std::mem::take(&mut *self.errors.lock().unwrap());
        if !errors.is_empty() {
            bail!("GPU validation failed: {}", errors.join("\n"));
        }
        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let native = if self.output.native_p3() {
            Some(NativeOutput::new(self.device.clone(), self._window.0, width.max(1), height.max(1))?)
        } else { None };
        self.check_errors()?;
        let mut config = self.config.clone();
        config.width = width.max(1);
        config.height = height.max(1);
        self.surface.configure(&self.device, &config);
        if let Err(error) = self.check_errors() {
            self.surface.configure(&self.device, &self.config);
            return Err(error);
        }
        self.config = config;
        self.native = native;
        Ok(())
    }

    pub fn output(&self) -> Output {
        self.output
    }

    pub fn capture_hdr_transport(&self) -> Result<Vec<u8>> {
        let Some(native) = &self.native else { bail!("HDR transport capture requires a float Screen render target"); };
        native.capture(&self.queue)
    }

    pub fn configure_output(
        &mut self,
        source_format: wgpu::TextureFormat,
        window_space: WorkingColorSpace,
    ) -> Result<()> {
        let output = Output::select(
            &self.surface.get_capabilities(&self.adapter), source_format, window_space,
        )?;
        if output == self.output() {
            return Ok(());
        }
        let pipeline = presentation::create_pipeline(&self.device, output.format);
        self.check_errors()?;
        let native = if output.native_p3() {
            Some(NativeOutput::new(self.device.clone(), self._window.0, self.config.width, self.config.height)?)
        } else { None };
        let mut config = self.config.clone();
        // Native HDR owns its own AHardwareBuffers. Keep the parent WSI surface
        // in a supported SDR configuration; never request extended P3 from WSI.
        if !output.native_p3() {
            config.format = output.format;
            config.color_space = output.color_space;
        }
        self.surface.configure(&self.device, &config);
        if let Err(error) = self.check_errors() {
            self.surface.configure(&self.device, &self.config);
            return Err(error);
        }
        log::info!("Screen output: {source_format:?} -> {output:?}");
        self.config = config;
        self.pipeline = pipeline;
        self.native = native;
        self.output = output;
        Ok(())
    }

    pub fn present(
        &self,
        texture: Option<&wgpu::TextureView>,
        resolution: [u32; 2],
        hdr_probe: bool,
    ) -> Result<bool> {
        let frame = if self.native.is_some() { None } else { Some(match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false);
            }
            error => bail!("Acquire Android Surface: {error:?}"),
        }) };
        self.queue.write_buffer(
            &self.probe,
            0,
            bytemuck::cast_slice(&self.output().options(
                hdr_probe, self.native_pixels, [self.config.width, self.config.height],
            )),
        );
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Scene presentation"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture.unwrap_or(&self.empty)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.probe.as_entire_binding(),
                },
            ],
        });
        let surface_view = frame.as_ref().map(|frame| frame.texture.create_view(&Default::default()));
        let view = self.native.as_ref().map(|native| &native.view).or(surface_view.as_ref()).unwrap();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Color-managed present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let [width, height] = resolution.map(|value| value.max(1) as f32);
            let scale = (self.config.width as f32 / width).min(self.config.height as f32 / height);
            let (w, h) = (width * scale, height * scale);
            if !self.native_pixels {
                pass.set_viewport(
                    (self.config.width as f32 - w) * 0.5,
                    (self.config.height as f32 - h) * 0.5,
                    w,
                    h,
                    0.0,
                    1.0,
                );
            }
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        if let Some(native) = &self.native {
            encoder.transition_resources(std::iter::empty(), std::iter::once(wgpu::TextureTransition {
                texture: &native.texture, selector: None, state: wgpu::TextureUses::COPY_SRC,
            }));
        }
        self.queue.submit([encoder.finish()]);
        if let Some(native) = &self.native { native.present(self.output.buffer_ratio())?; }
        if let Some(frame) = frame { self.queue.present(frame); }
        self.check_errors()?;
        Ok(true)
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }
}

struct NativeWindow(NonNull<ndk_sys::ANativeWindow>);
impl Drop for NativeWindow {
    fn drop(&mut self) {
        unsafe {
            ndk_sys::ANativeWindow_release(self.0.as_ptr());
        }
    }
}
