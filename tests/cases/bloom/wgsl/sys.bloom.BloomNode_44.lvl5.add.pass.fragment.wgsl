
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

    geo_translate: vec2f,
    geo_scale: vec2f,

    time: f32,
    _pad0: f32,

    color: vec4f,
    camera: mat4x4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) frag_coord_gl: vec2f,
    @location(2) local_px: vec3f,
    @location(3) geo_size_px: vec2f,
};

@group(1) @binding(0)
var base_tex: texture_2d<f32>;
@group(1) @binding(1)
var base_samp: sampler;
@group(1) @binding(2)
var add_tex: texture_2d<f32>;
@group(1) @binding(3)
var add_samp: sampler;

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let base = textureSample(base_tex, base_samp, in.uv);
    let add = textureSample(add_tex, add_samp, in.uv);
    // RGB is additive (HDR glow), alpha is coverage clamped to [0,1].
    return vec4f(base.rgb + add.rgb, clamp(base.a + add.a, 0.0, 1.0));
}
