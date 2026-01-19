
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

    geo_translate: vec2f,
    geo_scale: vec2f,

    // Pack to 16-byte boundary.
    time: f32,
    _pad0: f32,

    // 16-byte aligned.
    color: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
    @location(1) frag_coord_gl: vec2f,
};

// --- Extra WGSL declarations (generated) ---
fn mc_MathClosure_13_(uv: vec2<f32>, index: i32) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var index_1: i32;
    var output: vec3<f32> = vec3(0f);

    uv_1 = uv;
    index_1 = index;
    let _e8: i32 = index_1;
    output = vec3<f32>(0f, (f32(_e8) * 200f), 0f);
    let _e14: vec3<f32> = output;
    return _e14;
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return vec4f(params.color.rgb * params.color.a, params.color.a);
}
