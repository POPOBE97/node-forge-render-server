use crate::renderer::types::WgslShaderBundle;

/// Spec for fullscreen sampled fragment templates.
#[derive(Clone, Copy, Debug)]
pub struct FullscreenTemplateSpec {
    pub flip_y: bool,
}

impl Default for FullscreenTemplateSpec {
    fn default() -> Self {
        Self { flip_y: false }
    }
}

pub fn build_fullscreen_sampled_bundle(spec: FullscreenTemplateSpec) -> WgslShaderBundle {
    let sample_uv = if spec.flip_y {
        "let uv = nf_uv_pass(in.uv);"
    } else {
        "let uv = in.uv;"
    };

    crate::renderer::wgsl::build_fullscreen_textured_bundle(format!(
        "{sample_uv}\n    return textureSample(src_tex, src_samp, uv);"
    ))
}
