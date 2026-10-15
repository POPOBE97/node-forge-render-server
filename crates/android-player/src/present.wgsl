@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> options: vec4f;
struct Vertex { @builtin(position) position: vec4f, @location(0) uv: vec2f }
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> Vertex {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return Vertex(vec4f(p * 2.0 - 1.0, 0.0, 1.0), vec2f(p.x, 1.0 - p.y));
}
@fragment fn fs_main(input: Vertex) -> @location(0) vec4f {
    if (options.x > 0.5) {
        let level = select(select(0.25, 1.0, input.uv.x > 0.25), select(2.0, 4.0, input.uv.x > 0.75), input.uv.x > 0.5);
        return vec4f(vec3f(level), 1.0);
    }
    if (options.y > 0.5) {
        let size = vec2i(textureDimensions(scene));
        let origin = floor((options.zw - vec2f(size)) * 0.5);
        let pixel = vec2i(floor(input.position.xy - origin));
        if (any(pixel < vec2i(0)) || any(pixel >= size)) { return vec4f(0.0, 0.0, 0.0, 1.0); }
        return vec4f(textureLoad(scene, pixel, 0).rgb, 1.0);
    }
    // Scene textures are already linear, premultiplied and localized by the shared renderer.
    // The opaque Surface composites over black without SDR encoding or clipping highlights.
    let color = textureSample(scene, scene_sampler, input.uv);
    return vec4f(color.rgb, 1.0);
}
