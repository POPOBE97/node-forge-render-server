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
fn shader_material(in: ShaderMaterialInput) -> vec4f {
    return vec4f(in.uv, 0.0, 1.0);
}
