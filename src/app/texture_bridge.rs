use rust_wgpu_fiber::{
    ResourceName,
    eframe::{egui_wgpu, wgpu},
};

use super::types::App;

pub fn canvas_sampler_descriptor(filter: wgpu::FilterMode) -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("canvas_texture_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToBorder,
        address_mode_v: wgpu::AddressMode::ClampToBorder,
        address_mode_w: wgpu::AddressMode::ClampToBorder,
        border_color: Some(wgpu::SamplerBorderColor::TransparentBlack),
        mag_filter: filter,
        min_filter: filter,
        ..Default::default()
    }
}

pub fn diff_sampler_descriptor(filter: wgpu::FilterMode) -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToBorder,
        address_mode_v: wgpu::AddressMode::ClampToBorder,
        address_mode_w: wgpu::AddressMode::ClampToBorder,
        border_color: Some(wgpu::SamplerBorderColor::TransparentBlack),
        mag_filter: filter,
        min_filter: filter,
        ..Default::default()
    }
}

pub fn sync_output_texture(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    texture_name: &ResourceName,
    filter: wgpu::FilterMode,
) {
    let texture = app
        .shader_space
        .textures
        .get(texture_name.as_str())
        .unwrap_or_else(|| panic!("output texture not found: {}", texture_name));

    if let Some(id) = app.color_attachment {
        renderer.update_egui_texture_from_wgpu_texture_with_sampler_options(
            &render_state.device,
            texture.wgpu_texture_view.as_ref().unwrap(),
            canvas_sampler_descriptor(filter),
            id,
        );
    } else {
        app.color_attachment = Some(renderer.register_native_texture_with_sampler_options(
            &render_state.device,
            texture.wgpu_texture_view.as_ref().unwrap(),
            canvas_sampler_descriptor(filter),
        ));
    }
}

pub fn ensure_output_texture_registered(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
) {
    if app.color_attachment.is_none() {
        let name = app.output_texture_name.clone();
        sync_output_texture(app, render_state, renderer, &name, wgpu::FilterMode::Linear);
    }
}

/// Sync a preview texture (any named texture in the ShaderSpace) to an egui TextureId.
pub fn sync_preview_texture(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    texture_name: &ResourceName,
    filter: wgpu::FilterMode,
) {
    let texture = match app.shader_space.textures.get(texture_name.as_str()) {
        Some(t) => t,
        None => return,
    };
    let view = match texture.wgpu_texture_view.as_ref() {
        Some(v) => v,
        None => return,
    };

    if let Some(id) = app.preview_color_attachment {
        renderer.update_egui_texture_from_wgpu_texture_with_sampler_options(
            &render_state.device,
            view,
            canvas_sampler_descriptor(filter),
            id,
        );
    } else {
        app.preview_color_attachment = Some(renderer.register_native_texture_with_sampler_options(
            &render_state.device,
            view,
            canvas_sampler_descriptor(filter),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{canvas_sampler_descriptor, diff_sampler_descriptor};
    use rust_wgpu_fiber::eframe::wgpu;

    #[test]
    fn canvas_sampler_uses_transparent_border() {
        let sampler = canvas_sampler_descriptor(wgpu::FilterMode::Linear);
        assert_eq!(sampler.address_mode_u, wgpu::AddressMode::ClampToBorder);
        assert_eq!(sampler.address_mode_v, wgpu::AddressMode::ClampToBorder);
        assert_eq!(sampler.address_mode_w, wgpu::AddressMode::ClampToBorder);
        assert_eq!(
            sampler.border_color,
            Some(wgpu::SamplerBorderColor::TransparentBlack)
        );
    }

    #[test]
    fn diff_sampler_uses_transparent_border() {
        let sampler = diff_sampler_descriptor(wgpu::FilterMode::Linear);
        assert_eq!(sampler.address_mode_u, wgpu::AddressMode::ClampToBorder);
        assert_eq!(sampler.address_mode_v, wgpu::AddressMode::ClampToBorder);
        assert_eq!(sampler.address_mode_w, wgpu::AddressMode::ClampToBorder);
        assert_eq!(
            sampler.border_color,
            Some(wgpu::SamplerBorderColor::TransparentBlack)
        );
    }
}
