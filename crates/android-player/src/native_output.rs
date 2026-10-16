//! Native Android HDR transport. All GPU work is owned by the player's render thread.
use anyhow::{Context, Result, bail};
use ash::vk;
use rust_wgpu_fiber::wgpu;
use std::{
    ffi::{CStr, c_char, c_void},
    ptr::NonNull,
    sync::Arc,
};

type Vulkan = wgpu::hal::api::Vulkan;
const EXTENSIONS: [&CStr; 2] = [
    c"VK_ANDROID_external_memory_android_hardware_buffer",
    c"VK_EXT_queue_family_foreign",
];

unsafe extern "C" {
    fn nf_hdr_create(
        window: *mut ndk_sys::ANativeWindow,
        physical: vk::PhysicalDevice,
        device: vk::Device,
        queue: vk::Queue,
        family: u32,
        width: u32,
        height: u32,
        error: *mut c_char,
        size: usize,
    ) -> *mut c_void;
    fn nf_hdr_present(
        output: *mut c_void,
        source: vk::Image,
        ratio: f32,
        error: *mut c_char,
        size: usize,
    ) -> bool;
    fn nf_hdr_destroy(output: *mut c_void);
}

pub fn create_device(
    adapter: &wgpu::Adapter,
    desc: &wgpu::DeviceDescriptor<'_>,
) -> Result<(wgpu::Device, wgpu::Queue)> {
    // Extend wgpu's normal Vulkan device creation; do not replace its feature/limit setup.
    let hal = unsafe { adapter.as_hal::<Vulkan>() }.context("Android requires Vulkan")?;
    let supported = EXTENSIONS
        .iter()
        .all(|ext| hal.physical_device_capabilities().supports_extension(ext));
    let device = unsafe {
        hal.open_with_callback(
            desc.required_features,
            &desc.required_limits,
            &desc.memory_hints,
            Some(Box::new(move |args| {
                if supported {
                    for ext in EXTENSIONS {
                        if !args.extensions.contains(&ext) {
                            args.extensions.push(ext);
                        }
                    }
                }
            })),
        )?
    };
    drop(hal);
    Ok(unsafe { adapter.create_device_from_hal::<Vulkan>(device, desc) }?)
}

pub struct NativeOutput {
    raw: NonNull<c_void>,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    device: Arc<wgpu::Device>,
}

impl NativeOutput {
    pub fn new(
        device: Arc<wgpu::Device>,
        window: NonNull<ndk_sys::ANativeWindow>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let hal = unsafe { device.as_hal::<Vulkan>() }.context("Android requires Vulkan")?;
        if !EXTENSIONS
            .iter()
            .all(|ext| hal.enabled_device_extensions().contains(ext))
        {
            bail!("Android HDR requires Vulkan AHardwareBuffer import and foreign queue ownership");
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Android normalized HDR transport"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgb10a2Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut error = [0 as c_char; 512];
        let raw = unsafe {
            nf_hdr_create(
                window.as_ptr(),
                hal.raw_physical_device(),
                hal.raw_device().handle(),
                hal.raw_queue(),
                hal.queue_family_index(),
                width,
                height,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        let raw = NonNull::new(raw).ok_or_else(|| anyhow::anyhow!(error_string(&error)))?;
        drop(hal);
        let view = texture.create_view(&Default::default());
        Ok(Self {
            raw,
            texture,
            view,
            device,
        })
    }

    pub fn present(&self, ratio: f32) -> Result<()> {
        // wgpu has transitioned this image to COPY_SRC. Drain its submissions
        // before borrowing the same VkQueue; native copy waits on its own fence
        // before returning. No other thread submits to this player's queue.
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })?;
        let image =
            unsafe { self.texture.as_hal::<Vulkan>() }.context("HDR texture is not Vulkan")?;
        let mut error = [0 as c_char; 512];
        let success = unsafe {
            nf_hdr_present(
                self.raw.as_ptr(),
                image.raw_handle(),
                ratio,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if !success {
            bail!("{}", error_string(&error));
        }
        Ok(())
    }

    /// Read the exact packed 10-bit codes copied into the compositor buffers.
    pub fn capture(&self, queue: &wgpu::Queue) -> Result<Vec<u8>> {
        let row = self.texture.width() * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Android HDR transport capture"),
            size: padded as u64 * self.texture.height() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: None,
                },
            },
            self.texture.size(),
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })?;
        rx.recv()??;
        let mapped = buffer.slice(..).get_mapped_range()?;
        let mut bytes = Vec::with_capacity(row as usize * self.texture.height() as usize);
        for source in mapped.chunks_exact(padded as usize) {
            bytes.extend_from_slice(&source[..row as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(bytes)
    }
}

fn error_string(error: &[c_char]) -> String {
    // Native entry points always terminate the error buffer; it starts zeroed.
    unsafe { CStr::from_ptr(error.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

impl Drop for NativeOutput {
    fn drop(&mut self) {
        // Native destruction waits for the GPU, detaches the child layer, and
        // retains its buffer pool until compositor release callbacks complete.
        unsafe {
            nf_hdr_destroy(self.raw.as_ptr());
        }
    }
}
