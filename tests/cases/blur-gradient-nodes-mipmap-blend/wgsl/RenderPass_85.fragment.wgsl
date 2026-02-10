
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
    // Node: Vector2Input_89
    node_Vector2Input_89_b6d553bd: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_ImageTexture_24: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_ImageTexture_24: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_MathClosure_91_(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var output: vec2<f32> = vec2(0f);
    var padding: vec2<f32>;

    uv_1 = uv;
    xy_1 = xy;
    size_1 = size;
    let _e9: vec2<f32> = size_1;
    let _e16: vec2<f32> = size_1;
    padding = floor(((_e16 - vec2<f32>(1080f, 2400f)) * 0.5f));
    let _e25: vec2<f32> = xy_1;
    let _e26: vec2<f32> = padding;
    output = (((_e25 - _e26) - vec2<f32>(0.5f, -0.5f)) / vec2<f32>(1080f, 2400f));
    let _e37: vec2<f32> = output;
    return _e37;
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_91_out: vec2f;
    {
        let xy = in.frag_coord_gl;
        let size = in.geo_size_px;
        var output: vec2f;
        output = mc_MathClosure_91_(in.uv, xy, size);
        mc_MathClosure_91_out = output;
    }
    return textureSample(img_tex_ImageTexture_24, img_samp_ImageTexture_24, (mc_MathClosure_91_out));
}
