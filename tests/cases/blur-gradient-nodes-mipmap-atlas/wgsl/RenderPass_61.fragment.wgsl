
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
  };


struct GraphInputs {
    // Node: Vector2Input_83
    node_Vector2Input_83_e8df53bd: vec4f,
    // Node: Vector2Input_84
    node_Vector2Input_84_cdeb53bd: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_Downsample_18: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_Downsample_18: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_GroupInstance_64_MathClosure_30_(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var output: vec2<f32> = vec2(0f);

    uv_1 = uv;
    xy_1 = xy;
    size_1 = size;
    let _e9: vec2<f32> = xy_1;
    let _e10: vec2<f32> = size_1;
    output = (_e9 / _e10);
    let _e12: vec2<f32> = output;
    return _e12;
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_GroupInstance_64_MathClosure_30_out: vec2f;
    {
        let xy = in.local_px;
        let size = in.geo_size_px;
        var output: vec2f;
        output = mc_GroupInstance_64_MathClosure_30_(in.uv, xy, size);
        mc_GroupInstance_64_MathClosure_30_out = output;
    }
    return textureSample(pass_tex_Downsample_18, pass_samp_Downsample_18, vec2f((mc_GroupInstance_64_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_64_MathClosure_30_out).y));
}
