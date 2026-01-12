
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
let id = vec2f(in.position.xy);
let base = id * 8.0 + 0.5;
var c = vec4f(0.0);
for (var oy: f32 = 0.0; oy < 8.0; oy = oy + 2.0) {
    for (var ox: f32 = 0.0; ox < 8.0; ox = ox + 2.0) {
        c = c + textureSampleLevel(src_tex, src_samp, (base + vec2f(ox, oy)) / original, 0.0);
    }
}
return c * 0.0625;

}
