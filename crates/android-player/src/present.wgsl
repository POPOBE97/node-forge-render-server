@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
struct Options {
    viewport: vec4f, // probe, native pixels, surface width/height
    transfer: vec4f, // software sRGB OETF, buffer/SDR ratio, sRGB -> P3 transport, padding
}
@group(0) @binding(2) var<uniform> options: Options;

fn output_color(linear: vec3f) -> vec4f {
    var primaries = linear;
    if (options.transfer.z > 0.5) {
        // Screen's matrix is unbounded. Android's HDR transport uses P3 for both
        // window choices, so convert the represented color before normalization.
        primaries = mat3x3f(
            vec3f(0.82246196, 0.0331942, 0.01708263),
            vec3f(0.17753804, 0.9668058, 0.07239744),
            vec3f(0.0, 0.0, 0.9105199)
        ) * linear;
    }
    let transported = primaries / options.transfer.y;
    if (options.transfer.x > 0.5) {
        // Preserve extended/negative components until the final storage boundary.
        let magnitude = abs(transported);
        let encoded = select(1.055 * pow(magnitude, vec3f(1.0 / 2.4)) - 0.055,
                             12.92 * magnitude, magnitude <= vec3f(0.0031308));
        return vec4f(sign(transported) * encoded, 1.0);
    }
    return vec4f(transported, 1.0);
}
struct Vertex { @builtin(position) position: vec4f, @location(0) uv: vec2f }
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> Vertex {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return Vertex(vec4f(p * 2.0 - 1.0, 0.0, 1.0), vec2f(p.x, 1.0 - p.y));
}
@fragment fn fs_main(input: Vertex) -> @location(0) vec4f {
    if (options.viewport.x > 0.5) {
        let level = select(select(0.25, 1.0, input.uv.x > 0.25), select(2.0, 4.0, input.uv.x > 0.75), input.uv.x > 0.5);
        return output_color(vec3f(level));
    }
    if (options.viewport.y > 0.5) {
        let size = vec2i(textureDimensions(scene));
        let origin = floor((options.viewport.zw - vec2f(size)) * 0.5);
        let pixel = vec2i(floor(input.position.xy - origin));
        if (any(pixel < vec2i(0)) || any(pixel >= size)) { return vec4f(0.0, 0.0, 0.0, 1.0); }
        return output_color(textureLoad(scene, pixel, 0).rgb);
    }
    // Scene textures are already linear, premultiplied and localized by the shared renderer.
    // Composite over black in linear space, then encode for the configured Surface.
    let color = textureSample(scene, scene_sampler, input.uv);
    return output_color(color.rgb);
}
