fn shader_material(
    in: ShaderMaterialInput,
    content_tex: texture_2d<f32>,
    content_sampler: sampler,
    darken_alpha: f32,
) -> vec4f {
    let content = textureSample(content_tex, content_sampler, in.uv);
    let a = clamp(darken_alpha, 0.0, 1.0);
    return vec4f(content.rgb * (1.0 - a), a + content.a * (1.0 - a));
}
