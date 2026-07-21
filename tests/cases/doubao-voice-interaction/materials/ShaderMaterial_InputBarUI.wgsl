fn shader_material(
    in: ShaderMaterialInput,
    ui_color: vec4f,
    opacity: f32,
) -> vec4f {
    return ui_color * clamp(opacity, 0.0, 1.0);
}
