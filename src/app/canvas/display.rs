use rust_wgpu_fiber::eframe::{egui, egui_wgpu, wgpu};

use crate::app::{texture_bridge, types::App};

use super::pixel_overlay;

fn is_hdr_clamp_effective(
    hdr_preview_clamp_enabled: bool,
    texture_format: Option<wgpu::TextureFormat>,
) -> bool {
    hdr_preview_clamp_enabled && matches!(texture_format, Some(wgpu::TextureFormat::Rgba16Float))
}

pub struct DisplayFrame {
    pub effective_resolution: [u32; 2],
    pub compare_output_active: bool,
    pub display_texture_format: Option<wgpu::TextureFormat>,
    pub hdr_clamp_effective: bool,
    pub display_attachment: Option<egui::TextureId>,
    pub value_sampling_texture_name: String,
}

pub fn flush_deferred_frees(app: &mut App, renderer: &mut egui_wgpu::Renderer) {
    for id in app.canvas.display.deferred_texture_frees.drain(..) {
        renderer.free_texture(&id);
    }
}

pub fn sync_preview_source(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
) -> bool {
    if let Some(preview_name) = app.canvas.display.preview_texture_name.clone() {
        if app
            .shader_space
            .textures
            .contains_key(preview_name.as_str())
        {
            texture_bridge::sync_preview_texture(
                app,
                render_state,
                renderer,
                &preview_name,
                app.canvas.display.texture_filter,
            );
            true
        } else {
            app.canvas.display.preview_texture_name = None;
            if let Some(id) = app.canvas.display.preview_color_attachment.take() {
                app.canvas.display.deferred_texture_frees.push(id);
            }
            pixel_overlay::clear_cache(app);
            app.canvas.invalidation.preview_source_changed();
            false
        }
    } else {
        if let Some(id) = app.canvas.display.preview_color_attachment.take() {
            app.canvas.display.deferred_texture_frees.push(id);
        }
        false
    }
}

pub fn build_display_frame(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    using_preview: bool,
) -> DisplayFrame {
    let display_texture_name = if using_preview {
        app.canvas
            .display
            .preview_texture_name
            .as_ref()
            .map(|name| name.as_str().to_string())
            .unwrap_or_else(|| app.output_texture_name.as_str().to_string())
    } else {
        app.output_texture_name.as_str().to_string()
    };
    let effective_resolution = app
        .shader_space
        .texture_info(display_texture_name.as_str())
        .map(|info| [info.size.width, info.size.height])
        .unwrap_or(app.resolution);

    let compare_output_active = app.canvas.analysis.diff_texture_id.is_some();
    let display_texture_format = if compare_output_active {
        app.canvas
            .analysis
            .diff_renderer
            .as_ref()
            .map(|diff_renderer| diff_renderer.output_format())
    } else {
        None
    }
    .or_else(|| {
        app.shader_space
            .texture_info(display_texture_name.as_str())
            .map(|info| info.format)
    });
    let hdr_clamp_effective = is_hdr_clamp_effective(
        app.canvas.display.hdr_preview_clamp_enabled,
        display_texture_format,
    );

    let mut display_attachment = if compare_output_active {
        app.canvas.analysis.diff_texture_id
    } else if using_preview {
        app.canvas
            .display
            .preview_color_attachment
            .or(app.canvas.display.color_attachment)
    } else {
        app.canvas.display.color_attachment
    };

    if hdr_clamp_effective && !compare_output_active {
        let hdr_clamp_source = app
            .shader_space
            .textures
            .get(display_texture_name.as_str())
            .and_then(|texture| {
                texture.wgpu_texture_view.as_ref().map(|view| {
                    (
                        view.clone(),
                        [
                            texture.wgpu_texture_desc.size.width,
                            texture.wgpu_texture_desc.size.height,
                        ],
                    )
                })
            });

        if let Some((source_view, source_size)) = hdr_clamp_source {
            let clamp_renderer = app
                .canvas
                .display
                .hdr_clamp_renderer
                .get_or_insert_with(|| {
                    crate::ui::hdr_clamp::HdrClampRenderer::new(&render_state.device, source_size)
                });
            clamp_renderer.update(
                &render_state.device,
                &render_state.queue,
                &source_view,
                source_size,
            );

            let sampler =
                texture_bridge::canvas_sampler_descriptor(app.canvas.display.texture_filter);
            if let Some(id) = app.canvas.display.hdr_clamp_texture_id {
                renderer.update_egui_texture_from_wgpu_texture_with_sampler_options(
                    &render_state.device,
                    clamp_renderer.output_view(),
                    sampler,
                    id,
                );
            } else {
                app.canvas.display.hdr_clamp_texture_id =
                    Some(renderer.register_native_texture_with_sampler_options(
                        &render_state.device,
                        clamp_renderer.output_view(),
                        sampler,
                    ));
            }
            if let Some(id) = app.canvas.display.hdr_clamp_texture_id {
                display_attachment = Some(id);
            }
        }
    }

    DisplayFrame {
        effective_resolution,
        compare_output_active,
        display_texture_format,
        hdr_clamp_effective,
        display_attachment,
        value_sampling_texture_name: display_texture_name,
    }
}

#[cfg(test)]
mod tests {
    use super::is_hdr_clamp_effective;
    use rust_wgpu_fiber::eframe::wgpu;

    #[test]
    fn hdr_clamp_effective_requires_toggle_and_hdr_format() {
        assert!(is_hdr_clamp_effective(
            true,
            Some(wgpu::TextureFormat::Rgba16Float)
        ));
        assert!(!is_hdr_clamp_effective(
            false,
            Some(wgpu::TextureFormat::Rgba16Float)
        ));
        assert!(!is_hdr_clamp_effective(
            true,
            Some(wgpu::TextureFormat::Rgba8Unorm)
        ));
    }
}
