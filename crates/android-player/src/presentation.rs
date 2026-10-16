//! Android's final transfer encoding; Screen primary conversion lives in the shared graph.
use anyhow::{Result, bail};
use node_forge_render_server::color::WorkingColorSpace;
use rust_wgpu_fiber::wgpu;

// Transport peak in units of SDR white. This is unrelated to the scene working space.
pub const HDR_HEADROOM: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    pub format: wgpu::TextureFormat,
    pub color_space: wgpu::SurfaceColorSpace,
    pub window_space: WorkingColorSpace,
}

impl Output {
    pub fn select(
        caps: &wgpu::SurfaceCapabilities,
        source_format: wgpu::TextureFormat,
        window_space: WorkingColorSpace,
    ) -> Result<Self> {
        use wgpu::TextureFormat as F;
        let color_space = window_space.window_color_space(source_format);
        if matches!(
            color_space,
            wgpu::SurfaceColorSpace::ExtendedDisplayP3
                | wgpu::SurfaceColorSpace::ExtendedSrgbLinear
        ) {
            // Vulkan WSI has no extended-P3 enum. A native SurfaceControl declares
            // the range and brightness of our normalized 10-bit AHardwareBuffers.
            return Ok(Self {
                format: F::Rgb10a2Unorm,
                color_space: wgpu::SurfaceColorSpace::ExtendedDisplayP3,
                window_space,
            });
        }
        let required = color_space
            .to_color_spaces()
            .expect("explicit window space");
        let formats = &[F::Rgba8Unorm, F::Bgra8Unorm, F::Rgba8UnormSrgb, F::Bgra8UnormSrgb];
        for &format in formats {
            if caps.color_spaces(format).contains(required) {
                return Ok(Self {
                    format,
                    color_space,
                    window_space,
                });
            }
        }
        bail!(
            "Android Surface does not support {color_space:?} for source {source_format:?}. \
             Required formats: {formats:?}; supported: {:?}",
            caps.format_capabilities
        )
    }

    pub fn hdr(self) -> bool {
        self.native_p3()
    }

    pub fn native_p3(self) -> bool {
        self.color_space == wgpu::SurfaceColorSpace::ExtendedDisplayP3
    }

    pub fn buffer_ratio(self) -> f32 {
        if self.native_p3() { HDR_HEADROOM } else { 1.0 }
    }

    pub fn format_name(self) -> &'static str {
        match self.format {
            wgpu::TextureFormat::Rgba8Unorm => "rgba8unorm",
            wgpu::TextureFormat::Bgra8Unorm => "bgra8unorm",
            wgpu::TextureFormat::Rgba8UnormSrgb => "rgba8unorm-srgb",
            wgpu::TextureFormat::Bgra8UnormSrgb => "bgra8unorm-srgb",
            wgpu::TextureFormat::Rgb10a2Unorm => "rgb10a2unorm",
            _ => unreachable!("unsupported Android presentation format"),
        }
    }

    /// JNI contract: default sRGB = 0, wide-color SDR = 1, HDR = 2.
    pub fn android_color_mode(self) -> i32 {
        if self.hdr() {
            2
        } else if self.color_space == wgpu::SurfaceColorSpace::DisplayP3 {
            1
        } else {
            0
        }
    }

    pub fn color_space_name(self) -> &'static str {
        match self.color_space {
            wgpu::SurfaceColorSpace::Srgb => "srgb",
            wgpu::SurfaceColorSpace::DisplayP3 => "display-p3",
            wgpu::SurfaceColorSpace::ExtendedDisplayP3 => "extended-display-p3",
            _ => unreachable!("Output::select only accepts sRGB/P3"),
        }
    }

    pub fn options(self, probe: bool, native_pixels: bool, size: [u32; 2]) -> [f32; 8] {
        let encode = !self.format.is_srgb();
        [
            probe as u8 as f32,
            native_pixels as u8 as f32,
            size[0] as f32,
            size[1] as f32,
            encode as u8 as f32,
            self.buffer_ratio(),
            (self.native_p3() && self.window_space == WorkingColorSpace::Srgb) as u8 as f32,
            0.0,
        ]
    }
}

pub fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Android color-managed presentation"),
        source: wgpu::ShaderSource::Wgsl(include_str!("present.wgsl").into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Android color-managed presentation"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
