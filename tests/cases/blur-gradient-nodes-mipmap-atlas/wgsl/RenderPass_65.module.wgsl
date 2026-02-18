
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
     @location(2) local_px: vec3f,
     // Geometry size in pixels after applying geometry/instance transforms.
     @location(3) geo_size_px: vec2f,
  };


struct GraphInputs {
    // Node: Vector2Input_85
    node_Vector2Input_85_1aea53bd: vec4f,
    // Node: Vector2Input_86
    node_Vector2Input_86_67e853bd: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_Downsample_20: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_Downsample_20: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_GroupInstance_68_MathClosure_30_(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
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


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
 ) -> VSOut {
 var out: VSOut;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = uv;

 let rect_size_px_base = (graph_inputs.node_Vector2Input_85_1aea53bd).xy;
 let rect_center_px = (graph_inputs.node_Vector2Input_86_67e853bd).xy;
 let rect_dyn = vec4f(rect_center_px, rect_size_px_base);
 out.geo_size_px = rect_dyn.zw;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, 0.0);

 let p_rect_local_px = vec3f(position.xy * rect_dyn.zw, position.z);
 let p_local = p_rect_local_px;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = rect_dyn.xy + p_local.xy;

 // Convert pixels to clip space assuming bottom-left origin.
 // (0,0) => (-1,-1), (target_size) => (1,1)
 let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
 out.position = vec4f(ndc, p_local.z / params.target_size.x, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_GroupInstance_68_MathClosure_30_out: vec2f;
    {
        let xy = in.local_px.xy;
        let size = in.geo_size_px;
        var output: vec2f;
        output = mc_GroupInstance_68_MathClosure_30_(in.uv, xy, size);
        mc_GroupInstance_68_MathClosure_30_out = output;
    }
    return textureSample(pass_tex_Downsample_20, pass_samp_Downsample_20, vec2f((mc_GroupInstance_68_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_68_MathClosure_30_out).y));
}
