
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
    // Node: ColorInput_7
    node_ColorInput_7_fa5c7029: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;

// --- Extra WGSL declarations (generated) ---
fn mc_MathClosure_45_(uv: vec2<f32>, t: f32) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var t_1: f32;
    var output: vec3<f32> = vec3(0f);

    uv_1 = uv;
    t_1 = t;
    let _e8: f32 = t_1;
    let _e11: f32 = t_1;
    output = vec3<f32>(0f, (fract((_e11 / 5f)) * 180f), 0f);
    let _e19: vec3<f32> = output;
    return _e19;
}


fn sys_apply_trs_xyz(p: vec3f, t: vec3f, r_deg: vec3f, s: vec3f) -> vec3f {
    let rad = r_deg * 0.017453292519943295;

    let cx = cos(rad.x);
    let sx = sin(rad.x);
    let cy = cos(rad.y);
    let sy = sin(rad.y);
    let cz = cos(rad.z);
    let sz = sin(rad.z);

    let p0 = p * s;
    let p1 = vec3f(p0.x, p0.y * cx - p0.z * sx, p0.y * sx + p0.z * cx);
    let p2 = vec3f(p1.x * cy + p1.z * sy, p1.y, -p1.x * sy + p1.z * cy);
    let p3 = vec3f(p2.x * cz - p2.y * sz, p2.x * sz + p2.y * cz, p2.z);
    return p3 + t;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let _frag_out = vec4f((graph_inputs.node_ColorInput_7_fa5c7029).rgb * (graph_inputs.node_ColorInput_7_fa5c7029).a, (graph_inputs.node_ColorInput_7_fa5c7029).a);
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
