
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
    
let original = vec2f(textureDimensions(src_tex));
let xy = vec2f(in.position.xy);
let k = array<f32, 8>(0.066405416, 0.0868975, 0.083077468, 0.077410989, 0.070301622, 0.062225949, 0.053681057, 0);
let o = array<f32, 8>(0.66595155, 2.495979786, 4.492763996, 6.489548683, 8.486333847, 10.483120918, 12.479908943, 0);
var color = vec4f(0.0);
for (var i: u32 = 0u; i < 8u; i = i + 1u) {
    let uv_pos = (xy + vec2f(0.0, o[i])) / original;
    let uv_neg = (xy - vec2f(0.0, o[i])) / original;
    color = color + textureSampleLevel(src_tex, src_samp, uv_pos, 0.0) * k[i];
    color = color + textureSampleLevel(src_tex, src_samp, uv_neg, 0.0) * k[i];
}
return color;

}
