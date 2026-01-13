
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,
    time: f32,
    _pad0: f32,
    color: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};
@group(1) @binding(0)
var img_tex_node_15: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_node_15: sampler;


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return textureSample(img_tex_node_15, img_samp_node_15, vec2f((in.uv).x, 1.0 - (in.uv).y));
}
