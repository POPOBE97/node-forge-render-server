
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
    // Node: FloatInput_10
    node_FloatInput_10_157c0221: vec4f,
    // Node: FloatInput_107
    node_FloatInput_107_c6ed3817: vec4f,
    // Node: FloatInput_12
    node_FloatInput_12_af780221: vec4f,
    // Node: FloatInput_84
    node_FloatInput_84_38afe820: vec4f,
    // Node: FloatInput_89
    node_FloatInput_89_1faae820: vec4f,
    // Node: Vector2Input_119
    node_Vector2Input_119_4e6d6989: vec4f,
    // Node: Vector3Input_105
    node_Vector3Input_105_12e9923b: vec4f,
    // Node: Vector3Input_80
    node_Vector3Input_80_82af6f66: vec4f,
    // Node: dp.Sdf2DBevel_61.width
    node_dp_Sdf2DBevel_61_width_5ca037c2: vec4f,
    // Node: dp.Sdf2D_106.radius
    node_dp_Sdf2D_106_radius_60a95d0c: vec4f,
    // Node: dp.metric.MathClosure_115.falloffPx
    node_dp_metric_MathClosure_115_falloffPx_3eb1865f: vec4f,
    // Node: dp.metric.MathClosure_115.radiusPx
    node_dp_metric_MathClosure_115_radiusPx_f64ebaba: vec4f,
    // Node: dp.metric.Remap_112.edge1
    node_dp_metric_Remap_112_edge1_ebceb5e2: vec4f,
    // Node: dp.metric.Remap_113.edge0
    node_dp_metric_Remap_113_edge0_7b11e137: vec4f,
    // Node: dp.metric.Remap_113.edge1
    node_dp_metric_Remap_113_edge1_c80fe137: vec4f,
    // Node: dp.metric.Remap_64.edge1
    node_dp_metric_Remap_64_edge1_9f67a4c6: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_ImageTexture_76: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_ImageTexture_76: sampler;

@group(1) @binding(2)
var pass_tex_PassTexture_86: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_PassTexture_86: sampler;


// --- Extra WGSL declarations (generated) ---

fn aspect_correct_uv_fit(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    // r = image_aspect / geo_aspect; r > 1 means image is relatively wider than geometry.
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(max(1.0 / r, 1.0), max(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}
fn aspect_correct_uv_fill(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(min(1.0 / r, 1.0), min(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}

fn nf_math_closure_fragment_2eafeedf_closure(uv: vec2<f32>, n: vec3<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var n_1: vec3<f32>;
    var output: f32;

    uv_1 = uv;
    n_1 = n;
    let _e10: vec3<f32> = n_1;
    output = dot(_e10, vec3<f32>(0f, 0f, 1f));
    let _e16: f32 = output;
    return _e16;
}

fn nf_math_closure_fragment_30810be0_closure(uv: vec2<f32>, c: vec4<f32>, f: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_1: vec4<f32>;
    var f_1: f32;
    var output: vec4<f32>;

    uv_1 = uv;
    c_1 = c;
    f_1 = f;
    let _e7: vec4<f32> = c_1;
    let _e8: vec3<f32> = _e7.xyz;
    let _e9: vec4<f32> = c_1;
    let _e11: f32 = f_1;
    output = vec4<f32>(_e8.x, _e8.y, _e8.z, (_e9.w * _e11));
    let _e17: vec4<f32> = output;
    return _e17;
}

fn nf_math_closure_fragment_3da3ebdf_closure(uv: vec2<f32>, n: vec3<f32>, i: vec3<f32>, xy: vec2<f32>, size: vec2<f32>, ior: f32, depth: f32) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var n_1: vec3<f32>;
    var i_1: vec3<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var ior_1: f32;
    var depth_1: f32;
    var output: vec2<f32>;
    var o: vec3<f32>;
    var offset: vec2<f32>;

    uv_1 = uv;
    n_1 = n;
    i_1 = i;
    xy_1 = xy;
    size_1 = size;
    ior_1 = ior;
    depth_1 = depth;
    let _e18: vec3<f32> = i_1;
    let _e19: vec3<f32> = n_1;
    let _e20: f32 = ior_1;
    o = refract(_e18, _e19, _e20);
    let _e23: vec2<f32> = xy_1;
    let _e24: vec3<f32> = o;
    let _e26: f32 = depth_1;
    offset = (_e23 + (_e24.xy * _e26));
    let _e30: vec2<f32> = offset;
    let _e32: vec2<f32> = size_1;
    output = (_e30.xy / _e32);
    let _e34: vec2<f32> = output;
    return _e34;
}

fn nf_math_closure_fragment_3f7508e0_closure(uv: vec2<f32>, x: f32) -> f32 {
    var uv_1: vec2<f32>;
    var x_1: f32;
    var output: f32;

    uv_1 = uv;
    x_1 = x;
    let _e6: f32 = x_1;
    output = (1f - _e6);
    let _e8: f32 = output;
    return _e8;
}

fn nf_math_closure_fragment_7582d587_closure(uv: vec2<f32>, c: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_1: vec4<f32>;
    var output: vec4<f32>;

    uv_1 = uv;
    c_1 = c;
    output = vec4(0f);
    let _e7: vec4<f32> = c_1;
    if (_e7.w > 0f) {
        {
            let _e11: vec4<f32> = c_1;
            let _e13: vec4<f32> = c_1;
            let _e16: vec3<f32> = (_e11.xyz / vec3(_e13.w));
            let _e17: vec4<f32> = c_1;
            output = vec4<f32>(_e16.x, _e16.y, _e16.z, _e17.w);
        }
    }
    let _e23: vec4<f32> = output;
    return _e23;
}

fn nf_math_closure_fragment_825b1180_closure(uv_2: vec2<f32>, sampleUV: vec2<f32>) -> vec4<f32> {
    var uv_3: vec2<f32>;
    var sampleUV_1: vec2<f32>;
    var output: vec4<f32>;

    uv_3 = uv_2;
    sampleUV_1 = sampleUV;
    let _e6: vec2<f32> = sampleUV_1;
    let _e7: vec4<f32> = nf_math_closure_fragment_825b1180_closure_sample_texture_texture(_e6);
    output = _e7;
    let _e8: vec4<f32> = output;
    return _e8;
}

fn nf_math_closure_fragment_825b1180_closure_sample_texture_texture(uv: vec2f) -> vec4f {
    let raster_uv = vec2f(uv.x, 1.0 - uv.y);
    return textureSample(pass_tex_PassTexture_86, pass_samp_PassTexture_86, raster_uv);
}

fn nf_math_closure_fragment_9076d587_closure(uv: vec2<f32>, n: vec3<f32>, i: vec3<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var n_1: vec3<f32>;
    var i_1: vec3<f32>;
    var output: f32;
    var r: f32;

    uv_1 = uv;
    n_1 = n;
    i_1 = i;
    let _e8: vec3<f32> = n_1;
    n_1 = normalize(_e8);
    let _e11: vec3<f32> = n_1;
    let _e13: vec3<f32> = i_1;
    let _e15: vec3<f32> = n_1;
    let _e17: vec3<f32> = i_1;
    let _e21: vec3<f32> = n_1;
    let _e23: vec3<f32> = i_1;
    let _e25: vec3<f32> = n_1;
    let _e27: vec3<f32> = i_1;
    let _e32: vec3<f32> = n_1;
    let _e34: vec3<f32> = i_1;
    let _e36: vec2<f32> = -(_e34.xy);
    let _e37: vec3<f32> = i_1;
    let _e42: vec3<f32> = n_1;
    let _e44: vec3<f32> = i_1;
    let _e46: vec2<f32> = -(_e44.xy);
    let _e47: vec3<f32> = i_1;
    let _e54: vec3<f32> = n_1;
    let _e56: vec3<f32> = i_1;
    let _e58: vec2<f32> = -(_e56.xy);
    let _e59: vec3<f32> = i_1;
    let _e64: vec3<f32> = n_1;
    let _e66: vec3<f32> = i_1;
    let _e68: vec2<f32> = -(_e66.xy);
    let _e69: vec3<f32> = i_1;
    r = (max(0f, dot(_e25.xyz, _e27.xyz)) + max(0f, dot(_e64.xyz, vec3<f32>(_e68.x, _e68.y, _e69.z))));
    let _e78: f32 = r;
    output = (_e78 * 0.7f);
    let _e81: f32 = output;
    return _e81;
}

fn nf_math_closure_fragment_ad9a0be0_closure(uv: vec2<f32>, sampleUV: vec2<f32>, scale: f32) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var sampleUV_1: vec2<f32>;
    var scale_1: f32;
    var output: vec2<f32>;

    uv_1 = uv;
    sampleUV_1 = sampleUV;
    scale_1 = scale;
    let _e7: vec2<f32> = sampleUV_1;
    let _e11: f32 = scale_1;
    output = (((_e7 - vec2(0.5f)) * _e11) + vec2(0.5f));
    let _e16: vec2<f32> = output;
    return _e16;
}

fn nf_math_closure_fragment_b2bbd787_closure(uv: vec2<f32>, t: f32, c: vec4<f32>, thumb: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var t_1: f32;
    var c_1: vec4<f32>;
    var thumb_1: f32;
    var output: vec4<f32>;
    var r: vec4<f32>;

    uv_1 = uv;
    t_1 = t;
    c_1 = c;
    thumb_1 = thumb;
    let _e9: vec4<f32> = c_1;
    let _e10: f32 = t_1;
    r = (_e9 * _e10);
    let _e17: vec4<f32> = r;
    let _e20: f32 = thumb_1;
    r = mix(_e17, vec4(1f), vec4(_e20));
    let _e23: vec4<f32> = r;
    output = _e23;
    let _e24: vec4<f32> = output;
    return _e24;
}

fn nf_math_closure_fragment_c06c08e0_closure(uv: vec2<f32>, c_edge: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_edge_1: vec4<f32>;
    var output: vec4<f32>;

    uv_1 = uv;
    c_edge_1 = c_edge;
    let _e6: vec4<f32> = c_edge_1;
    c_edge_1.w = (_e6.w * 0.05f);
    let _e10: vec4<f32> = c_edge_1;
    output = _e10;
    let _e11: vec4<f32> = output;
    return _e11;
}

fn nf_math_closure_fragment_c280d587_closure(uv: vec2<f32>, c: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_1: vec4<f32>;
    var output: vec4<f32>;

    uv_1 = uv;
    c_1 = c;
    output = vec4(0f);
    let _e7: vec4<f32> = c_1;
    if (_e7.w > 0f) {
        {
            let _e11: vec4<f32> = c_1;
            let _e13: vec4<f32> = c_1;
            let _e16: vec3<f32> = (_e11.xyz / vec3(_e13.w));
            let _e17: vec4<f32> = c_1;
            output = vec4<f32>(_e16.x, _e16.y, _e16.z, _e17.w);
        }
    }
    let _e23: vec4<f32> = output;
    return _e23;
}

fn nf_math_closure_fragment_closure(uv: vec2<f32>, c_edge: vec4<f32>, e: f32, c_ui: vec4<f32>, f: f32, l: f32, selection: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_edge_1: vec4<f32>;
    var e_1: f32;
    var c_ui_1: vec4<f32>;
    var f_1: f32;
    var l_1: f32;
    var selection_1: f32;
    var output: vec4<f32>;
    var edge_color: vec4<f32>;
    var alpha_r: f32;
    var color_r: vec3<f32>;
    var r: vec4<f32>;

    uv_1 = uv;
    c_edge_1 = c_edge;
    e_1 = e;
    c_ui_1 = c_ui;
    f_1 = f;
    l_1 = l;
    selection_1 = selection;
    let _e15: vec4<f32> = c_edge_1;
    let _e16: vec3<f32> = _e15.xyz;
    let _e17: vec4<f32> = c_edge_1;
    let _e19: f32 = e_1;
    let _e21: f32 = f_1;
    edge_color = vec4<f32>(_e16.x, _e16.y, _e16.z, ((_e17.w * _e19) * _e21));
    let _e29: vec4<f32> = edge_color;
    let _e32: f32 = l_1;
    let _e33: f32 = f_1;
    let _e38: f32 = selection_1;
    let _e39: f32 = f_1;
    let _e44: f32 = e_1;
    edge_color.w = (_e29.w + (((0.15f + ((_e32 * _e33) * 0.6f)) + ((_e38 * _e39) * 0.2f)) * _e44));
    let _e47: vec4<f32> = edge_color;
    let _e49: vec4<f32> = edge_color;
    let _e53: f32 = l_1;
    let _e54: f32 = f_1;
    let _e56: f32 = selection_1;
    let _e57: f32 = f_1;
    let _e60: vec4<f32> = edge_color;
    let _e64: f32 = l_1;
    let _e65: f32 = f_1;
    let _e67: f32 = selection_1;
    let _e68: f32 = f_1;
    let _e72: vec3<f32> = mix(_e60.xyz, vec3(1f), vec3(((_e64 * _e65) + (_e67 * _e68))));
    edge_color.x = _e72.x;
    edge_color.y = _e72.y;
    edge_color.z = _e72.z;
    let _e79: vec4<f32> = edge_color;
    let _e81: vec4<f32> = c_ui_1;
    let _e84: vec4<f32> = edge_color;
    alpha_r = (_e79.w + (_e81.w * (1f - _e84.w)));
    let _e90: vec4<f32> = edge_color;
    let _e92: vec4<f32> = edge_color;
    let _e95: vec4<f32> = c_ui_1;
    let _e97: vec4<f32> = c_ui_1;
    let _e101: vec4<f32> = edge_color;
    let _e106: f32 = alpha_r;
    color_r = (((_e90.xyz * _e92.w) + ((_e95.xyz * _e97.w) * (1f - _e101.w))) / vec3(_e106));
    let _e110: vec3<f32> = color_r;
    let _e111: f32 = alpha_r;
    r = vec4<f32>(_e110.x, _e110.y, _e110.z, _e111);
    let _e117: vec4<f32> = r;
    output = _e117;
    let _e118: vec4<f32> = output;
    return _e118;
}

fn nf_math_closure_fragment_d78208e0_closure(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var output: vec2<f32>;

    uv_1 = uv;
    xy_1 = xy;
    size_1 = size;
    let _e7: vec2<f32> = xy_1;
    let _e8: vec2<f32> = size_1;
    output = (_e7 / _e8);
    let _e10: vec2<f32> = output;
    return _e10;
}

fn nf_math_closure_fragment_e6b4d787_closure(uv: vec2<f32>, sdf: f32, radiusPx: f32, falloffPx: f32) -> f32 {
    var uv_1: vec2<f32>;
    var sdf_1: f32;
    var radiusPx_1: f32;
    var falloffPx_1: f32;
    var output: f32;
    var r: f32;

    uv_1 = uv;
    sdf_1 = sdf;
    radiusPx_1 = radiusPx;
    falloffPx_1 = falloffPx;
    let _e9: f32 = sdf_1;
    let _e10: f32 = radiusPx_1;
    let _e13: f32 = sdf_1;
    let _e14: f32 = radiusPx_1;
    r = max((_e13 + _e14), 0f);
    let _e19: f32 = r;
    let _e20: f32 = falloffPx_1;
    r = (_e19 / _e20);
    let _e22: f32 = r;
    let _e24: f32 = r;
    let _e26: f32 = r;
    let _e28: f32 = r;
    output = f32(exp((-(_e26) * _e28)));
    let _e32: f32 = output;
    return _e32;
}

fn nf_math_closure_fragment_f48ad587_closure(uv: vec2<f32>, t: f32, size: vec2<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var t_1: f32;
    var size_1: vec2<f32>;
    var output: vec2<f32>;

    uv_1 = uv;
    t_1 = t;
    size_1 = size;
    let _e7: vec2<f32> = size_1;
    let _e9: f32 = t_1;
    let _e11: vec2<f32> = size_1;
    output = vec2<f32>((_e7.x * _e9), (_e11.y / 2f));
    let _e16: vec2<f32> = output;
    return _e16;
}


// ---- 2D SDF bevel helpers (generated) ----
// 2D SDF bevel helper template.
//
// This file is the editable WGSL source for Sdf2DBevel curve helper functions.
// The Rust compiler wires node inputs into calls to these helpers.

fn sdf2d_bevel_smooth5_map(t_in: f32) -> f32 {
    // Evaluate the symmetric polynomial in the nearer endpoint's coordinate.
    // Expanded terms near t=1 lose precision before finite-difference normals.
    let t = clamp(0.5 + t_in * 0.5, 0.0, 1.0);
    let q = min(t, 1.0 - t);
    let polynomial = q * q * q * (10.0 + q * (-15.0 + 6.0 * q));
    return select(2.0 * polynomial - 1.0, 1.0 - 2.0 * polynomial, t >= 0.5);
}

fn sdf2d_bevel_smooth5(d_in: f32, edge: f32, cliff: f32) -> f32 {
    var d = d_in;
    if (d < -edge) {
        d = -edge;
    } else if (d < 0.0) {
        var x = -d / edge;
        if (x >= 0.85) {
            x = 1.0;
        } else {
            x = clamp(x, 0.0, 1.0);
            x = sdf2d_bevel_smooth5_map(pow(x, 0.5));
            x = 1.0 - pow(1.0 - x, cliff);
        }
        d = -x * edge;
    }
    return d;
}

fn sdf2d_bevel_smooth7_map(t_in: f32) -> f32 {
    let t = clamp(0.5 + t_in * 0.5, 0.0, 1.0);
    let q = min(t, 1.0 - t);
    let q2 = q * q;
    let polynomial = q2 * q2 * (35.0 + q * (-84.0 + q * (70.0 - 20.0 * q)));
    return select(2.0 * polynomial - 1.0, 1.0 - 2.0 * polynomial, t >= 0.5);
}

fn sdf2d_bevel_smooth7(d_in: f32, edge: f32, cliff: f32) -> f32 {
    var d = d_in;
    if (d < -edge) {
        d = -edge;
    } else if (d < 0.0) {
        var x = -d / edge;
        if (x >= 0.85) {
            x = 1.0;
        } else {
            x = clamp(x, 0.0, 1.0);
            x = sdf2d_bevel_smooth7_map(pow(x, 0.5));
            x = 1.0 - pow(1.0 - x, cliff);
        }
        d = -x * edge;
    }
    return d;
}

fn sdf2d_bevel_eps() -> f32 {
    return 0.002;
}

fn sdf2d_bevel_normal(depth_px: f32, depth_nx: f32, depth_py: f32, depth_ny: f32, eps: f32) -> vec3f {
    let safe_eps = max(abs(eps), 1e-6);
    let dx = (depth_px - depth_nx) / (2.0 * safe_eps);
    let dy = (depth_py - depth_ny) / (2.0 * safe_eps);
    return normalize(vec3f(-dx, -dy, 1.0));
}

// Note: normal reconstruction uses 4 extra evaluations (finite differences).
// Potential optimization: use `dpdx`/`dpdy` in WGSL to estimate derivatives with fewer calls.


// ---- 2D SDF helpers (generated) ----
// 2D SDF helper template.
//
// This file is the editable WGSL source for Sdf2D shape helper functions.
// The Rust compiler wires node inputs into calls to these helpers.

fn sdf2d_round_rect(p: vec2f, b: vec2f, rad4: vec4f) -> f32 {
    var r: f32 = rad4.x;
    if (p.x > 0.0 && p.y > 0.0) {
        r = rad4.y;
    } else if (p.x > 0.0 && p.y < 0.0) {
        r = rad4.z;
    } else if (p.x < 0.0 && p.y < 0.0) {
        r = rad4.w;
    }

    let q = abs(p) - b + vec2f(r, r);
    let outside = length(max(q, vec2f(0.0, 0.0)));
    let inside = min(max(q.x, q.y), 0.0);
    return outside + inside - r;
}

fn sdf2d_smooth_round_rect(point: vec2f, center: vec2f, radius: f32, axis_mix: vec2f) -> vec3f {
    let abs_radius = abs(radius);
    let scaled_radius = 1.5286649465560913 * abs_radius;
    let safe_scaled_radius = max(scaled_radius, 1e-6);
    let blended_radius = mix(scaled_radius, radius, max(axis_mix.x, axis_mix.y));

    let offset = point - center;
    let shifted_pos = vec2f(safe_scaled_radius, safe_scaled_radius) + offset;
    let normalized_pos = max(vec2f(0.0), shifted_pos / safe_scaled_radius);
    let abs_norm_pos = abs(normalized_pos);

    let axis_denom = max(abs_norm_pos.x, abs_norm_pos.y);
    let axis_ratio = select(
        clamp(min(abs_norm_pos.x, abs_norm_pos.y) / axis_denom, 0.0, 1.0),
        0.0,
        axis_denom == 0.0,
    );

    let poly_fit_0 = -0.7391197269 * axis_ratio + 2.4034927648;
    let poly_fit_1 = poly_fit_0 * axis_ratio - 2.4907319173;
    let poly_fit_2 = poly_fit_1 * axis_ratio + 0.4768708960;
    let poly_fit = poly_fit_2 * axis_ratio + 0.4747847594;
    let len_abs = length(abs_norm_pos);
    let denom = 1.0 - axis_ratio * axis_ratio * clamp(len_abs, 0.0, 1.0) * poly_fit;
    let safe_denom = select(denom, 1e-6, abs(denom) < 1e-6);
    let dist_base = (len_abs + 1.0) - 1.0 / safe_denom;
    let dist_alt_pos = max(
        vec2f(0.0),
        1.5286649465560913 * abs_norm_pos - vec2f(0.5286650061607361),
    );
    let dist_alt = 0.6541655659675598 * length(dist_alt_pos) + 0.3458344340324402;

    let dist_mix_x = mix(dist_base, dist_alt, axis_mix.x);
    let dist_mix_y = mix(dist_base, dist_alt, axis_mix.y);
    let axis_sign = select(-1.0, 1.0, abs_norm_pos.y > abs_norm_pos.x);
    let final_mix = mix(dist_mix_x, dist_mix_y, clamp(0.5 - axis_sign + axis_sign * axis_ratio, 0.0, 1.0));

    let radial_pos = vec2f(blended_radius, blended_radius) + offset;
    let dir_norm = normalize(max(vec2f(0.0), radial_pos));
    let fallback_axis = select(vec2f(0.0, 1.0), vec2f(1.0, 0.0), radial_pos.x > radial_pos.y);
    let fallback_dir = select(fallback_axis, dir_norm, dir_norm.x + dir_norm.y > 0.0);
    let final_height = min(max(radial_pos.x, radial_pos.y), 0.0) + safe_scaled_radius * (final_mix - 1.0);

    return vec3f(final_height, fallback_dir);
}


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
     @builtin(instance_index) instance_index: u32,
 ) -> VSOut {
 var out: VSOut;

 out.instance_index = instance_index;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = vec2f(uv.x, 1.0 - uv.y);

 let rect_size_px_base = vec2f((graph_inputs.node_Vector2Input_119_4e6d6989).x, (graph_inputs.node_Vector2Input_119_4e6d6989).y);
 let rect_center_px = baked_data_parse[(instance_index) * 1u + 0u].xy;
 let rect_dyn = vec4f(rect_center_px, rect_size_px_base);
 out.geo_size_px = rect_dyn.zw;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(uv * out.geo_size_px, 0.0);

 let p_rect_local_px = vec3f(position.xy * rect_dyn.zw, position.z);
 var p_local = p_rect_local_px;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = rect_dyn.xy + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }