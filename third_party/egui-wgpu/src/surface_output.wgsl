@group(0) @binding(0) var linear_composition: texture_2d<f32>;

@vertex fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let positions = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    return vec4f(positions[index], 0.0, 1.0);
}

@fragment fn fs_main(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let c = textureLoad(linear_composition, vec2i(position.xy), 0);
    // Encode straight RGB, then restore premultiplication. Preserve extended RGB,
    // alpha, and additive RGB at alpha=0. Never apply a gamut clamp or tone map.
    let divisor = select(1.0, c.a, c.a > 0.0);
    let straight = c.rgb / divisor;
    let magnitude = abs(straight);
    let encoded = select(
        1.055 * pow(magnitude, vec3f(1.0 / 2.4)) - 0.055,
        12.92 * magnitude,
        magnitude <= vec3f(0.0031308)
    );
    return vec4f(sign(straight) * encoded * divisor, c.a);
}
