// MeshGradient vertex template.

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
    camera_position: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
    @location(2) frag_coord_gl: vec2f,
    @location(3) local_px: vec3f,
    @location(4) geo_size_px: vec2f,
};

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return in.color;
}
