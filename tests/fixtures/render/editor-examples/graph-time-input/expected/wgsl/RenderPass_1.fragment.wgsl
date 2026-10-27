
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
    camera: mat4x4f,
    camera_position: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

 struct VSOut {
     @builtin(position) position: vec4f,
     @location(0) uv: vec2f,
     // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
     @location(1) frag_coord_gl: vec2f,
     // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
     @location(2) local_px: vec3f,
     // Geometry size in pixels after applying geometry/instance transforms.
     @location(3) geo_size_px: vec2f,
     @location(4) @interpolate(flat) instance_index: u32,
 };


struct GraphInputs {
    // Node: dp.Rect2DGeometry_5.size
    node_dp_Rect2DGeometry_5_size_1c348e92: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;

// --- Extra WGSL declarations (generated) ---
fn nf_math_closure_fragment_closure(uv: vec2<f32>, input1_: vec2<f32>, input2_: vec2<f32>, input3_: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var input1_1: vec2<f32>;
    var input2_1: vec2<f32>;
    var input3_1: f32;
    var output: vec4<f32>;

    uv_1 = uv;
    input1_1 = input1_;
    input2_1 = input2_;
    input3_1 = input3_;
    let _e9: vec2<f32> = input1_1;
    let _e10: vec2<f32> = input2_1;
    let _e11: vec2<f32> = (_e9 / _e10);
    let _e12: f32 = input3_1;
    let _e15: f32 = input3_1;
    let _e21: f32 = input3_1;
    let _e24: f32 = input3_1;
    output = vec4<f32>(_e11.x, _e11.y, abs((fract((_e24 / 2f)) - 0.5f)), 1f);
    let _e35: vec4<f32> = output;
    return _e35;
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let math_closure_out = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), in.local_px.xy, in.geo_size_px, params.time);
    // Final composite
    let _frag_out = math_closure_out;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
