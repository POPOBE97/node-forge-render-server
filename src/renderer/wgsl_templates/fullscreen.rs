use crate::renderer::types::WgslShaderBundle;

pub fn build_fullscreen_sampled_bundle() -> WgslShaderBundle {
    crate::renderer::wgsl::build_fullscreen_textured_bundle(format!(
        "return textureSample(src_tex, src_samp, in.uv);"
    ))
}
