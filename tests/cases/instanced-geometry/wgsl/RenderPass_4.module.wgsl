
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
    @location(2) instance_index: u32,
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
    let _e29: vec3<f32> = gap_1;
    let _e30: i32 = x;
    let _e31: i32 = y;
    output = (_e29 * vec3<f32>(f32(_e30), f32(_e31), 0f));
    let _e37: vec3<f32> = output;
    return _e37;
}


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
     @location(2) i0: vec4f,
     @location(3) i1: vec4f,
     @location(4) i2: vec4f,
     @location(5) i3: vec4f,
     @builtin(instance_index) instance_index: u32,
 ) -> VSOut {
 var out: VSOut;

    var mc_MathClosure_18_out: vec3f;
    {
        let index = i32(instance_index);
        let gap = vec3f(200.78, 200, 0);
        var output: vec3f;
        output = mc_MathClosure_18_(uv, index, gap);
        mc_MathClosure_18_out = output;
    }
 out.instance_index = instance_index;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = uv;

 let inst_m = mat4x4f(i0, i1, i2, i3);
 var p_local = (inst_m * vec4f(position, 1.0)).xyz;

 let delta_t = mc_MathClosure_18_out;
 p_local = p_local + delta_t;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 let p_px = params.center + p_local.xy + (params.target_size * 0.5);

 // Convert pixels to clip space assuming bottom-left origin.
 // (0,0) => (-1,-1), (target_size) => (1,1)
 let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
 out.position = vec4f(ndc, position.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return vec4f(params.color.rgb * params.color.a, params.color.a);
}
