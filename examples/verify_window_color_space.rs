//! Native smoke check: configure a real window, switch SDR and extended spaces,
//! verify the applied surface configuration, and close. Run manually on a GPU host:
//! cargo run --locked --example verify_window_color_space
use rust_wgpu_fiber::eframe::{self, egui, egui_wgpu, wgpu};

struct Check {
    stage: usize,
    frames: usize,
}

impl eframe::App for Check {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let spaces = [
            wgpu::SurfaceColorSpace::ExtendedSrgbLinear,
            wgpu::SurfaceColorSpace::Srgb,
            wgpu::SurfaceColorSpace::DisplayP3,
            wgpu::SurfaceColorSpace::ExtendedDisplayP3,
            wgpu::SurfaceColorSpace::ExtendedSrgbLinear,
            wgpu::SurfaceColorSpace::DisplayP3,
            wgpu::SurfaceColorSpace::Srgb,
            wgpu::SurfaceColorSpace::ExtendedDisplayP3,
        ];
        let Some(&expected) = spaces.get(self.stage) else {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        let render_state = frame.wgpu_render_state().unwrap();
        render_state.request_surface_color_space(expected).unwrap();
        ui.heading("Window output verification");
        ui.label(format!("{expected:?}"));
        ui.colored_label(egui::Color32::RED, "sRGB UI red");
        ui.colored_label(egui::Color32::GREEN, "sRGB UI green");
        self.frames += 1;
        if self.frames >= 3 {
            let applied = render_state.active_surface_output.read().unwrap();
            assert_eq!(applied.color_space, expected);
            assert_eq!(applied.format == wgpu::TextureFormat::Rgba16Float, expected.is_hdr());
            println!("Applied window surface: {applied:?}");
            self.frames = 0;
            self.stage += 1;
            if self.stage == spaces.len() {
                println!("Native SDR/extended sRGB/P3 switching verified");
            }
        }
        ui.ctx().request_repaint();
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "Node Forge output verification",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([340.0, 160.0]),
            renderer: eframe::Renderer::Wgpu,
            wgpu_options: egui_wgpu::WgpuConfiguration {
                preferred_surface_format: Some(wgpu::TextureFormat::Rgba16Float),
                preferred_surface_color_space: Some(wgpu::SurfaceColorSpace::ExtendedSrgbLinear),
                ..Default::default()
            },
            ..Default::default()
        },
        Box::new(|_| Ok(Box::new(Check { stage: 0, frames: 0 }))),
    )
}
