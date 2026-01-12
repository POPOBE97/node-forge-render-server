
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
var src_tex: texture_2d<f32>;
@group(1) @binding(1)
var src_samp: sampler;

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    
let src_size = vec2f(textureDimensions(src_tex));
let offset = vec2f(1.5 / src_size.x, 1.5 / src_size.y);
let uv = vec2f(in.uv.x, 1.0 - in.uv.y) + offset;
return textureSampleLevel(src_tex, src_samp, uv, 0.0);

}
