
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
    
let src_resolution = vec2f(textureDimensions(src_tex));
let dst_xy = vec2f(in.position.xy);
let base = dst_xy * 8.0 - vec2f(3.5);

var sum = vec4f(0.0);
for (var y: i32 = 0; y < 8; y = y + 1) {
    for (var x: i32 = 0; x < 8; x = x + 1) {
        let uv = (base + vec2f(f32(x), f32(y))) / src_resolution;
        sum = sum + textureSampleLevel(src_tex, src_samp, uv, 0.0);
    }
}

return sum * (1.0 / 64.0);

}
