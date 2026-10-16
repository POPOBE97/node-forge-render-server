// Public geometry is centered LocalPx, Y up. Atlas UV is private storage space.
struct Params {
    target_size: vec2f, geo_size: vec2f, center: vec2f,
    geo_translate: vec2f, geo_scale: vec2f, time: f32, _pad0: f32,
    color: vec4f, camera: mat4x4f, camera_position: vec4f,
};
@group(0) @binding(0) var<uniform> params: Params;
@group(1) @binding(0) var glyph_atlas: texture_2d<f32>;
@group(1) @binding(1) var glyph_sampler: sampler;
struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) atlas_uv: vec2f,
    @location(1) local_px: vec2f,
};
@vertex fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
    var out: VSOut;
    out.position = params.camera * vec4f(params.center + position.xy, position.z, 1.0);
    out.atlas_uv = uv;
    out.local_px = position.xy + params.geo_size * 0.5;
    return out;
}
@fragment fn fs_main(in: VSOut) -> @location(0) vec4f {
    let coverage = textureSample(glyph_atlas, glyph_sampler, in.atlas_uv).a;
    if any(in.local_px < vec2f(0.0)) || any(in.local_px >= params.geo_size) { discard; }
    let alpha = coverage * params.color.a;
    return vec4f(params.color.rgb * alpha, alpha);
}
