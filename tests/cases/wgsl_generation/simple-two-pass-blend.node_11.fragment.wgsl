
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
var img_tex_node_8: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_node_8: sampler;


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return textureSample(img_tex_node_8, img_samp_node_8, in.uv);
}
