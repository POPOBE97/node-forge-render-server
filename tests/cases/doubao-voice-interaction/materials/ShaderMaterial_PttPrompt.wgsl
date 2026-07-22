fn shader_material(
    in: ShaderMaterialInput,
    send_color: vec4f,
    cancel_color: vec4f,
    prompt_color: vec4f,
    prompt_opacity: f32,
    cancel_mix: f32,
) -> vec4f {
    let mask_alpha = mix(send_color.a, cancel_color.a, clamp(cancel_mix, 0.0, 1.0));
    let coverage = clamp(mask_alpha * prompt_color.a * prompt_opacity, 0.0, 1.0);
    return vec4f(prompt_color.rgb * 1.5 * coverage, coverage);
}