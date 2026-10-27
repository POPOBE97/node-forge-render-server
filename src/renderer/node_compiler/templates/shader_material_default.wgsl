// ShaderMaterialInput is provided by Node Forge.
//
// struct ShaderMaterialInput {
//     uv: vec2f,
//     frag_coord: vec2f,
//     local_position: vec3f,
//     geometry_size: vec2f,
//     target_size: vec2f,
//     time: f32,
// };
//
// Add user parameters after `in`; Node Forge reflects them into input ports.
// Example: fn shader_material(in: ShaderMaterialInput, gain: f32) -> vec4f
//
// To add a texture input, declare adjacent texture/sampler parameters:
// fn shader_material(
//     in: ShaderMaterialInput,
//     image: texture_2d<f32>,
//     image_sampler: sampler,
// ) -> vec4f {
//     return sample_texture_local_uv(image, image_sampler, in.uv);
// }
// The sampler must immediately follow its texture. This pair creates an `image`
// texture input port (`resource:image`). Use sample_texture_local_uv for
// bottom-left/Y-up LocalUV coordinates; do not flip Y yourself.
fn shader_material(in: ShaderMaterialInput) -> vec4f {
    return vec4f(in.uv, 0.0, 1.0);
}
