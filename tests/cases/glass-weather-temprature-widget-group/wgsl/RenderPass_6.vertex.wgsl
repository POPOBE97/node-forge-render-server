
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

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_GroupInstance_128_ImageTexture_76: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_GroupInstance_128_ImageTexture_76: sampler;

@group(1) @binding(2)
var pass_tex_GroupInstance_128_GuassianBlurPass_85: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_GroupInstance_128_GuassianBlurPass_85: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_GroupInstance_128_MathClosure_102_(uv: vec2<f32>, c: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_1: vec4<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    c_1 = c;
    let _e7: vec4<f32> = c_1;
    let _e9: vec4<f32> = c_1;
    let _e12: vec3<f32> = (_e7.xyz / vec3(_e9.w));
    let _e13: vec4<f32> = c_1;
    output = vec4<f32>(_e12.x, _e12.y, _e12.z, _e13.w);
    let _e19: vec4<f32> = output;
    return _e19;
}

fn mc_GroupInstance_128_MathClosure_103_(uv: vec2<f32>, c: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_1: vec4<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    c_1 = c;
    let _e7: vec4<f32> = c_1;
    let _e9: vec4<f32> = c_1;
    let _e12: vec3<f32> = (_e7.xyz / vec3(_e9.w));
    let _e13: vec4<f32> = c_1;
    output = vec4<f32>(_e12.x, _e12.y, _e12.z, _e13.w);
    let _e19: vec4<f32> = output;
    return _e19;
}

fn mc_GroupInstance_128_MathClosure_104_(uv: vec2<f32>, n: vec3<f32>, i: vec3<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var n_1: vec3<f32>;
    var i_1: vec3<f32>;
    var output: f32 = 0f;
    var r: f32;

    uv_1 = uv;
    n_1 = n;
    i_1 = i;
    let _e10: vec3<f32> = n_1;
    n_1 = normalize(_e10);
    let _e13: vec3<f32> = n_1;
    let _e15: vec3<f32> = i_1;
    let _e17: vec3<f32> = n_1;
    let _e19: vec3<f32> = i_1;
    let _e23: vec3<f32> = n_1;
    let _e25: vec3<f32> = i_1;
    let _e27: vec3<f32> = n_1;
    let _e29: vec3<f32> = i_1;
    let _e34: vec3<f32> = n_1;
    let _e36: vec3<f32> = i_1;
    let _e38: vec2<f32> = -(_e36.xy);
    let _e39: vec3<f32> = i_1;
    let _e44: vec3<f32> = n_1;
    let _e46: vec3<f32> = i_1;
    let _e48: vec2<f32> = -(_e46.xy);
    let _e49: vec3<f32> = i_1;
    let _e56: vec3<f32> = n_1;
    let _e58: vec3<f32> = i_1;
    let _e60: vec2<f32> = -(_e58.xy);
    let _e61: vec3<f32> = i_1;
    let _e66: vec3<f32> = n_1;
    let _e68: vec3<f32> = i_1;
    let _e70: vec2<f32> = -(_e68.xy);
    let _e71: vec3<f32> = i_1;
    r = (max(0f, dot(_e27.xyz, _e29.xyz)) + max(0f, dot(_e66.xyz, vec3<f32>(_e70.x, _e70.y, _e71.z))));
    let _e80: f32 = r;
    output = (_e80 * 0.7f);
    let _e83: f32 = output;
    return _e83;
}

fn mc_GroupInstance_128_MathClosure_108_(uv: vec2<f32>, t: f32, size: vec2<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var t_1: f32;
    var size_1: vec2<f32>;
    var output: vec2<f32> = vec2(0f);

    uv_1 = uv;
    t_1 = t;
    size_1 = size;
    let _e9: vec2<f32> = size_1;
    let _e11: f32 = t_1;
    let _e13: vec2<f32> = size_1;
    output = vec2<f32>((_e9.x * _e11), (_e13.y / 2f));
    let _e18: vec2<f32> = output;
    return _e18;
}

fn mc_GroupInstance_128_MathClosure_111_(uv: vec2<f32>, t: f32, c: vec4<f32>, thumb: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var t_1: f32;
    var c_1: vec4<f32>;
    var thumb_1: f32;
    var output: vec4<f32> = vec4(0f);
    var r: vec4<f32>;

    uv_1 = uv;
    t_1 = t;
    c_1 = c;
    thumb_1 = thumb;
    let _e11: vec4<f32> = c_1;
    let _e12: f32 = t_1;
    r = (_e11 * _e12);
    let _e19: vec4<f32> = r;
    let _e22: f32 = thumb_1;
    r = mix(_e19, vec4(1f), vec4(_e22));
    let _e25: vec4<f32> = r;
    output = _e25;
    let _e26: vec4<f32> = output;
    return _e26;
}

fn mc_GroupInstance_128_MathClosure_115_(uv: vec2<f32>, sdf: f32) -> f32 {
    var uv_1: vec2<f32>;
    var sdf_1: f32;
    var output: f32 = 0f;
    var r: f32;

    uv_1 = uv;
    sdf_1 = sdf;
    let _e7: f32 = sdf_1;
    let _e11: f32 = sdf_1;
    r = max((_e11 + 22f), 0f);
    let _e17: f32 = r;
    r = (_e17 / 42f);
    let _e20: f32 = r;
    let _e22: f32 = r;
    let _e24: f32 = r;
    let _e26: f32 = r;
    output = f32(exp((-(_e24) * _e26)));
    let _e30: f32 = output;
    return _e30;
}

fn mc_GroupInstance_128_MathClosure_63_(uv: vec2<f32>, n: vec3<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var n_1: vec3<f32>;
    var output: f32 = 0f;

    uv_1 = uv;
    n_1 = n;
    let _e12: vec3<f32> = n_1;
    output = dot(_e12, vec3<f32>(0f, 0f, 1f));
    let _e18: f32 = output;
    return _e18;
}

fn mc_GroupInstance_128_MathClosure_79_(uv: vec2<f32>, n: vec3<f32>, i: vec3<f32>, xy: vec2<f32>, size: vec2<f32>, ior: f32, depth: f32) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var n_1: vec3<f32>;
    var i_1: vec3<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var ior_1: f32;
    var depth_1: f32;
    var output: vec2<f32> = vec2(0f);
    var o: vec3<f32>;
    var offset: vec2<f32>;

    uv_1 = uv;
    n_1 = n;
    i_1 = i;
    xy_1 = xy;
    size_1 = size;
    ior_1 = ior;
    depth_1 = depth;
    let _e20: vec3<f32> = i_1;
    let _e21: vec3<f32> = n_1;
    let _e22: f32 = ior_1;
    o = refract(_e20, _e21, _e22);
    let _e25: vec2<f32> = xy_1;
    let _e26: vec3<f32> = o;
    let _e28: f32 = depth_1;
    offset = (_e25 + (_e26.xy * _e28));
    let _e32: vec2<f32> = offset;
    let _e34: vec2<f32> = size_1;
    output = (_e32.xy / _e34);
    let _e36: vec2<f32> = output;
    return _e36;
}

fn mc_GroupInstance_128_MathClosure_87_(uv: vec2<f32>, c: vec4<f32>, f: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_1: vec4<f32>;
    var f_1: f32;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    c_1 = c;
    f_1 = f;
    let _e9: vec4<f32> = c_1;
    let _e10: vec3<f32> = _e9.xyz;
    let _e11: vec4<f32> = c_1;
    let _e13: f32 = f_1;
    output = vec4<f32>(_e10.x, _e10.y, _e10.z, (_e11.w * _e13));
    let _e19: vec4<f32> = output;
    return _e19;
}

fn mc_GroupInstance_128_MathClosure_88_(uv: vec2<f32>, uv_1: vec2<f32>, scale: f32) -> vec2<f32> {
    var uv_2: vec2<f32>;
    var uv_3: vec2<f32>;
    var scale_1: f32;
    var output: vec2<f32> = vec2(0f);

    uv_2 = uv;
    uv_3 = uv_1;
    scale_1 = scale;
    let _e9: vec2<f32> = uv_3;
    let _e13: f32 = scale_1;
    output = (((_e9 - vec2(0.5f)) * _e13) + vec2(0.5f));
    let _e18: vec2<f32> = output;
    return _e18;
}

fn mc_GroupInstance_128_MathClosure_91_(uv: vec2<f32>, x: f32) -> f32 {
    var uv_1: vec2<f32>;
    var x_1: f32;
    var output: f32 = 0f;

    uv_1 = uv;
    x_1 = x;
    let _e8: f32 = x_1;
    output = (1f - _e8);
    let _e10: f32 = output;
    return _e10;
}

fn mc_GroupInstance_128_MathClosure_94_(uv: vec2<f32>, c_edge: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_edge_1: vec4<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    c_edge_1 = c_edge;
    let _e8: vec4<f32> = c_edge_1;
    c_edge_1.w = (_e8.w * 0.05f);
    let _e12: vec4<f32> = c_edge_1;
    output = _e12;
    let _e13: vec4<f32> = output;
    return _e13;
}

fn mc_GroupInstance_128_MathClosure_96_(uv: vec2<f32>, c_edge: vec4<f32>, e: f32, c_ui: vec4<f32>, f: f32, l: f32, selection: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_edge_1: vec4<f32>;
    var e_1: f32;
    var c_ui_1: vec4<f32>;
    var f_1: f32;
    var l_1: f32;
    var selection_1: f32;
    var output: vec4<f32> = vec4(0f);
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
    let _e17: vec4<f32> = c_edge_1;
    let _e18: vec3<f32> = _e17.xyz;
    let _e19: vec4<f32> = c_edge_1;
    let _e21: f32 = e_1;
    let _e23: f32 = f_1;
    edge_color = vec4<f32>(_e18.x, _e18.y, _e18.z, ((_e19.w * _e21) * _e23));
    let _e31: vec4<f32> = edge_color;
    let _e34: f32 = l_1;
    let _e35: f32 = f_1;
    let _e40: f32 = selection_1;
    let _e41: f32 = f_1;
    let _e46: f32 = e_1;
    edge_color.w = (_e31.w + (((0.15f + ((_e34 * _e35) * 0.6f)) + ((_e40 * _e41) * 0.2f)) * _e46));
    let _e49: vec4<f32> = edge_color;
    let _e51: vec4<f32> = edge_color;
    let _e55: f32 = l_1;
    let _e56: f32 = f_1;
    let _e58: f32 = selection_1;
    let _e59: f32 = f_1;
    let _e62: vec4<f32> = edge_color;
    let _e66: f32 = l_1;
    let _e67: f32 = f_1;
    let _e69: f32 = selection_1;
    let _e70: f32 = f_1;
    let _e74: vec3<f32> = mix(_e62.xyz, vec3(1f), vec3(((_e66 * _e67) + (_e69 * _e70))));
    edge_color.x = _e74.x;
    edge_color.y = _e74.y;
    edge_color.z = _e74.z;
    let _e81: vec4<f32> = edge_color;
    let _e83: vec4<f32> = c_ui_1;
    let _e86: vec4<f32> = edge_color;
    alpha_r = (_e81.w + (_e83.w * (1f - _e86.w)));
    let _e92: vec4<f32> = edge_color;
    let _e94: vec4<f32> = edge_color;
    let _e97: vec4<f32> = c_ui_1;
    let _e99: vec4<f32> = c_ui_1;
    let _e103: vec4<f32> = edge_color;
    let _e108: f32 = alpha_r;
    color_r = (((_e92.xyz * _e94.w) + ((_e97.xyz * _e99.w) * (1f - _e103.w))) / vec3(_e108));
    let _e112: vec3<f32> = color_r;
    let _e113: f32 = alpha_r;
    r = vec4<f32>(_e112.x, _e112.y, _e112.z, _e113);
    let _e119: vec4<f32> = r;
    output = _e119;
    let _e120: vec4<f32> = output;
    return _e120;
}

fn mc_GroupInstance_128_MathClosure_99_(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
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

fn sdf2d_bevel_smooth5_map(t_in: f32) -> f32 {
    // Map t in [0, 1] into a symmetric [-1, 1] curve.
    var t = 0.5 + t_in * 0.5;
    t = clamp(t, 0.0, 1.0);
    // 5th-degree smootherstep: t^3 * (t * (t * 6 - 15) + 10)
    t = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    return (t - 0.5) * 2.0;
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
            x = sdf2d_bevel_smooth5_map(x);
            x = pow(x, cliff);
        }
        d = -x * edge;
    }
    return d;
}

fn sdf2d_bevel_smooth7_map(t_in: f32) -> f32 {
    // Map t in [0, 1] into a symmetric [-1, 1] curve.
    var t = 0.5 + t_in * 0.5;
    t = clamp(t, 0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let t5 = t4 * t;
    let t6 = t5 * t;
    let t7 = t6 * t;
    // 7th-degree smooth polynomial
    t = -20.0 * t7 + 70.0 * t6 - 84.0 * t5 + 35.0 * t4;
    return (t - 0.5) * 2.0;
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
            x = sdf2d_bevel_smooth7_map(x);
            x = pow(x, cliff);
        }
        d = -x * edge;
    }
    return d;
}

// Note: normal reconstruction below uses 4 extra evaluations (finite differences).
// Potential optimization: use `dpdx`/`dpdy` in WGSL to estimate derivatives with fewer calls.

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

 out.geo_size_px = params.geo_size;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = uv * out.geo_size_px;

 let p_local = position;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 let p_px = params.center + p_local.xy;

 // Convert pixels to clip space assuming bottom-left origin.
 // (0,0) => (-1,-1), (target_size) => (1,1)
 let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
 out.position = vec4f(ndc, position.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }