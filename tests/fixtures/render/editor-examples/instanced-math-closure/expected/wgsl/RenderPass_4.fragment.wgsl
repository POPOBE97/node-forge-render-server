
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
     // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
     @location(2) local_px: vec2f,
     // Geometry size in pixels after applying geometry/instance transforms.
     @location(3) geo_size_px: vec2f,
     @location(4) instance_index: u32,
 };

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;

// --- Extra WGSL declarations (generated) ---
fn mc_MathClosure_18_(uv: vec2<f32>, index: i32, gap: vec3<f32>) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var index_1: i32;
    var gap_1: vec3<f32>;
    var output: vec3<f32> = vec3(0f);
    var row: i32 = 5i;
    var col: i32 = 10i;
    var x: i32;
    var y: i32;
    var base: vec3<f32> = vec3<f32>(540f, 1200f, 0f);

    uv_1 = uv;
    index_1 = index;
    gap_1 = gap;
    let _e13: i32 = index_1;
    let _e14: i32 = row;
    let _e16: i32 = row;
    x = ((_e13 % _e14) - (_e16 / 2i));
    let _e21: i32 = index_1;
    let _e22: i32 = col;
    let _e24: i32 = col;
    y = ((_e21 / _e22) - (_e24 / 2i));
    let _e34: vec3<f32> = gap_1;
    let _e35: i32 = x;
    let _e36: i32 = y;
    let _e42: vec3<f32> = base;
    output = ((_e34 * vec3<f32>(f32(_e35), f32(_e36), 0f)) + _e42);
    let _e44: vec3<f32> = output;
    return _e44;
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return vec4f(params.color.rgb * params.color.a, params.color.a);
}
