
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
    // Node: BoolInput_140
    node_BoolInput_140_13a147c1: vec4i,
    // Node: FloatInput_137
    node_FloatInput_137_f1da2f17: vec4f,
    // Node: GroupInstance_132/FloatInput_10
    node_GroupInstance_132_FloatInput_10_d7a4e6d9: vec4f,
    // Node: GroupInstance_132/FloatInput_12
    node_GroupInstance_132_FloatInput_12_3da8e6d9: vec4f,
    // Node: GroupInstance_132/FloatInput_89
    node_GroupInstance_132_FloatInput_89_b1b7ccd9: vec4f,
    // Node: GroupInstance_132/Vector3Input_105
    node_GroupInstance_132_Vector3Input_105_b41cb78d: vec4f,
    // Node: GroupInstance_132/Vector3Input_80
    node_GroupInstance_132_Vector3Input_80_2831bc4b: vec4f,
    // Node: Vector2Input_143
    node_Vector2Input_143_9dd97189: vec4f,
    // Node: Vector2Input_146
    node_Vector2Input_146_1ed17189: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_GroupInstance_132_ImageTexture_76: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_GroupInstance_132_ImageTexture_76: sampler;

@group(1) @binding(2)
var pass_tex_GroupInstance_132_GuassianBlurPass_85: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_GroupInstance_132_GuassianBlurPass_85: sampler;


// --- Extra WGSL declarations (generated) ---

// ---- ColorMix (Blend Color) helpers (generated) ----

fn blendColorBurnComponent(src: vec2f, dst: vec2f) -> f32 {
    let t = select(0.0, dst.y, dst.y == dst.x);
    let d = select(
        t,
        dst.y - min(dst.y, (dst.y - dst.x) * src.y / (src.x + 0.001)),
        abs(src.x) > 0.0,
    );
    return (d * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn blendColorDodgeComponent(src: vec2f, dst: vec2f) -> f32 {
    let dxScale = select(1.0, 0.0, dst.x == 0.0);
    let delta = dxScale * min(
        dst.y,
        select(dst.y, (dst.x * src.y) / ((src.y - src.x) + 0.001), abs(src.y - src.x) > 0.0),
    );
    return (delta * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn blendOverlayComponent(src: vec2f, dst: vec2f) -> f32 {
    return select(
        src.y * dst.y - (2.0 * (dst.y - dst.x)) * (src.y - src.x),
        (2.0 * src.x) * dst.x,
        2.0 * dst.x <= dst.y,
    );
}

fn blendSoftLightComponent(src: vec2f, dst: vec2f) -> f32 {
    let EPSILON = 0.0;

    if (2.0 * src.x <= src.y) {
        return (((dst.x * dst.x) * (src.y - 2.0 * src.x)) / (dst.y + EPSILON) +
            (1.0 - dst.y) * src.x) +
            dst.x * ((-src.y + 2.0 * src.x) + 1.0);
    } else if (4.0 * dst.x <= dst.y) {
        let dSqd = dst.x * dst.x;
        let dCub = dSqd * dst.x;
        let daSqd = dst.y * dst.y;
        let daCub = daSqd * dst.y;

        return (((daSqd * (src.x - dst.x * ((3.0 * src.y - 6.0 * src.x) - 1.0)) +
            ((12.0 * dst.y) * dSqd) * (src.y - 2.0 * src.x)) -
            (16.0 * dCub) * (src.y - 2.0 * src.x)) -
            daCub * src.x) / (daSqd + EPSILON);
    } else {
        return ((dst.x * ((src.y - 2.0 * src.x) + 1.0) + src.x) -
            sqrt(dst.y * dst.x) * (src.y - 2.0 * src.x)) -
            dst.y * src.x;
    }
}

fn blendColorSaturation(color: vec3f) -> f32 {
    return max(max(color.x, color.y), color.z) - min(min(color.x, color.y), color.z);
}

fn blendHSLColor(flipSat: vec2f, src: vec4f, dst: vec4f) -> vec4f {
    let EPSILON = 0.0;
    let MIN_NORMAL_HALF = 6.10351562e-05;

    let alpha = dst.a * src.a;
    let sda = src.rgb * dst.a;
    let dsa = dst.rgb * src.a;

    let flip_x = flipSat.x != 0.0;
    let flip_y = flipSat.y != 0.0;

    var l = select(sda, dsa, flip_x);
    var r = select(dsa, sda, flip_x);

    if (flip_y) {
        let mn = min(min(l.x, l.y), l.z);
        let mx = max(max(l.x, l.y), l.z);
        l = select(vec3f(0.0), ((l - mn) * blendColorSaturation(r)) / (mx - mn), mx > mn);
        r = dsa;
    }

    let lum = dot(vec3f(0.3, 0.59, 0.11), r);
    var result = (lum - dot(vec3f(0.3, 0.59, 0.11), l)) + l;

    let minComp = min(min(result.x, result.y), result.z);
    let maxComp = max(max(result.x, result.y), result.z);

    if (minComp < 0.0 && lum != minComp) {
        result = lum + (result - lum) * (lum / ((lum - minComp + MIN_NORMAL_HALF) + EPSILON));
    }
    if (maxComp > alpha && maxComp != lum) {
        result = lum + ((result - lum) * (alpha - lum)) / ((maxComp - lum + MIN_NORMAL_HALF) + EPSILON);
    }

    return vec4f(
        ((result + dst.rgb) - dsa + src.rgb) - sda,
        src.a + dst.a - alpha,
    );
}

fn blendNormal(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb * (1.0 - src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendDarken(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - max(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendMultiply(src: vec4f, dst: vec4f) -> vec4f {
    return src * (1.0 - dst.a) + dst * (1.0 - src.a) + src * dst;
}

fn blendPlusDarker(src: vec4f, dst: vec4f) -> vec4f {
    let a = src.a + (1.0 - src.a) * dst.a;
    let color = max(vec3f(0.0), a - (dst.a - dst.rgb) - (src.a - src.rgb));
    return vec4f(color, a);
}

fn blendColorBurn(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        blendColorBurnComponent(src.ra, dst.ra),
        blendColorBurnComponent(src.ga, dst.ga),
        blendColorBurnComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendLighten(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendScreen(src: vec4f, dst: vec4f) -> vec4f {
    return vec4f(1.0 - (1.0 - src.rgb) * (1.0 - dst.rgb), src.a + dst.a * (1.0 - src.a));
}

fn blendPlusLighter(src: vec4f, dst: vec4f) -> vec4f {
    let color = min(src.rgb + dst.rgb, vec3f(1.0));
    let alpha = src.a + (1.0 - src.a) * dst.a;
    return vec4f(color, alpha);
}

fn blendColorDodge(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        blendColorDodgeComponent(src.ra, dst.ra),
        blendColorDodgeComponent(src.ga, dst.ga),
        blendColorDodgeComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendOverlay(src: vec4f, dst: vec4f) -> vec4f {
    var c = vec3f(
        blendOverlayComponent(src.ra, dst.ra),
        blendOverlayComponent(src.ga, dst.ga),
        blendOverlayComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    c += dst.rgb * (1.0 - src.a) + src.rgb * (1.0 - dst.a);
    return vec4f(c, a);
}

fn blendSoftLight(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        blendSoftLightComponent(src.ra, dst.ra),
        blendSoftLightComponent(src.ga, dst.ga),
        blendSoftLightComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendHardLight(src: vec4f, dst: vec4f) -> vec4f {
    return blendOverlay(dst, src);
}

fn blendDifference(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - 2.0 * min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendExclusion(src: vec4f, dst: vec4f) -> vec4f {
    let c = (dst.rgb + src.rgb) - (2.0 * dst.rgb * src.rgb);
    let a = src.a + (1.0 - src.a) * dst.a;
    return vec4f(c, a);
}

fn blendHue(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(0.0, 1.0), src, dst);
}

fn blendSaturation(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(1.0), src, dst);
}

fn blendColor(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(0.0), src, dst);
}

fn blendLuminance(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(1.0, 0.0), src, dst);
}

fn mc_GroupInstance_132_GroupInstance_125_MathClosure_99_(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
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

fn mc_GroupInstance_132_MathClosure_104_(uv: vec2<f32>, n: vec3<f32>, i: vec3<f32>) -> f32 {
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

fn mc_GroupInstance_132_MathClosure_108_(uv: vec2<f32>, t: f32, size: vec2<f32>) -> vec2<f32> {
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

fn mc_GroupInstance_132_MathClosure_111_(uv: vec2<f32>, t: f32, c_ui: vec4<f32>, thumb: f32, show_thumb: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var t_1: f32;
    var c_ui_1: vec4<f32>;
    var thumb_1: f32;
    var show_thumb_1: f32;
    var output: vec4<f32> = vec4(0f);
    var r: vec4<f32>;

    uv_1 = uv;
    t_1 = t;
    c_ui_1 = c_ui;
    thumb_1 = thumb;
    show_thumb_1 = show_thumb;
    let _e13: vec4<f32> = c_ui_1;
    let _e18: f32 = t_1;
    let _e19: f32 = show_thumb_1;
    r = (_e13 * mix(1f, _e18, _e19));
    let _e26: f32 = thumb_1;
    let _e27: f32 = show_thumb_1;
    let _e29: vec4<f32> = r;
    let _e32: f32 = thumb_1;
    let _e33: f32 = show_thumb_1;
    r = mix(_e29, vec4(1f), vec4((_e32 * _e33)));
    let _e37: vec4<f32> = r;
    output = _e37;
    let _e38: vec4<f32> = output;
    return _e38;
}

fn mc_GroupInstance_132_MathClosure_115_(uv: vec2<f32>, sdf: f32, show_thumb: f32) -> f32 {
    var uv_1: vec2<f32>;
    var sdf_1: f32;
    var show_thumb_1: f32;
    var output: f32 = 0f;
    var r: f32;

    uv_1 = uv;
    sdf_1 = sdf;
    show_thumb_1 = show_thumb;
    let _e9: f32 = sdf_1;
    let _e13: f32 = sdf_1;
    r = max((_e13 + 22f), 0f);
    let _e19: f32 = r;
    r = (_e19 / 42f);
    let _e22: f32 = r;
    let _e24: f32 = r;
    let _e26: f32 = r;
    let _e28: f32 = r;
    let _e32: f32 = show_thumb_1;
    output = (f32(exp((-(_e26) * _e28))) * _e32);
    let _e34: f32 = output;
    return _e34;
}

fn mc_GroupInstance_132_MathClosure_63_(uv: vec2<f32>, n: vec3<f32>) -> f32 {
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

fn mc_GroupInstance_132_MathClosure_79_(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>, depth: f32, refract_offset: vec3<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var depth_1: f32;
    var refract_offset_1: vec3<f32>;
    var output: vec2<f32> = vec2(0f);
    var offset: vec2<f32>;

    uv_1 = uv;
    xy_1 = xy;
    size_1 = size;
    depth_1 = depth;
    refract_offset_1 = refract_offset;
    let _e13: vec2<f32> = xy_1;
    let _e14: vec3<f32> = refract_offset_1;
    let _e16: f32 = depth_1;
    offset = (_e13 + (_e14.xy * _e16));
    let _e20: vec2<f32> = offset;
    let _e22: vec2<f32> = size_1;
    output = (_e20.xy / _e22);
    let _e24: vec2<f32> = output;
    return _e24;
}

fn mc_GroupInstance_132_MathClosure_88_(uv: vec2<f32>, uv_1: vec2<f32>, scale: f32) -> vec2<f32> {
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

fn mc_GroupInstance_132_MathClosure_91_(uv: vec2<f32>, x: f32) -> f32 {
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

fn mc_GroupInstance_132_MathClosure_96_(uv: vec2<f32>, c_edge: vec4<f32>, e: f32, f: f32, l: f32, selection: f32, lumin_edge: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_edge_1: vec4<f32>;
    var e_1: f32;
    var f_1: f32;
    var l_1: f32;
    var selection_1: f32;
    var lumin_edge_1: f32;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    c_edge_1 = c_edge;
    e_1 = e;
    f_1 = f;
    l_1 = l;
    selection_1 = selection;
    lumin_edge_1 = lumin_edge;
    let _e17: vec4<f32> = c_edge_1;
    let _e21: vec4<f32> = c_edge_1;
    let _e26: vec4<f32> = c_edge_1;
    let _e28: f32 = lumin_edge_1;
    let _e30: vec3<f32> = mix(vec3(1f), _e26.xyz, vec3(_e28));
    c_edge_1.x = _e30.x;
    c_edge_1.y = _e30.y;
    c_edge_1.z = _e30.z;
    let _e38: vec4<f32> = c_edge_1;
    let _e40: f32 = e_1;
    let _e41: f32 = f_1;
    c_edge_1.w = (_e38.w * ((_e40 * _e41) * 0.05f));
    let _e47: vec4<f32> = c_edge_1;
    let _e54: f32 = lumin_edge_1;
    let _e56: f32 = l_1;
    let _e57: f32 = f_1;
    let _e62: f32 = selection_1;
    let _e63: f32 = f_1;
    let _e68: f32 = e_1;
    c_edge_1.w = (_e47.w + (((mix(0.08f, 0.22f, _e54) + ((_e56 * _e57) * 0.6f)) + ((_e62 * _e63) * 0.2f)) * _e68));
    let _e71: vec4<f32> = c_edge_1;
    let _e73: vec4<f32> = c_edge_1;
    let _e77: f32 = l_1;
    let _e78: f32 = f_1;
    let _e80: f32 = selection_1;
    let _e81: f32 = f_1;
    let _e84: vec4<f32> = c_edge_1;
    let _e88: f32 = l_1;
    let _e89: f32 = f_1;
    let _e91: f32 = selection_1;
    let _e92: f32 = f_1;
    let _e96: vec3<f32> = mix(_e84.xyz, vec3(1f), vec3(((_e88 * _e89) + (_e91 * _e92))));
    c_edge_1.x = _e96.x;
    c_edge_1.y = _e96.y;
    c_edge_1.z = _e96.z;
    let _e103: vec4<f32> = c_edge_1;
    let _e105: vec4<f32> = c_edge_1;
    let _e107: vec4<f32> = c_edge_1;
    let _e109: vec3<f32> = (_e105.xyz * _e107.w);
    c_edge_1.x = _e109.x;
    c_edge_1.y = _e109.y;
    c_edge_1.z = _e109.z;
    let _e116: vec4<f32> = c_edge_1;
    output = _e116;
    let _e117: vec4<f32> = output;
    return _e117;
}

fn nf_premultiply(c: vec4f) -> vec4f {
    return vec4f(c.rgb * c.a, c.a);
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


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_GroupInstance_132_MathClosure_79_out: vec2f;
    {
        let xy = in.local_px;
        let size = in.geo_size_px;
        let depth = sdf2d_bevel_smooth5(sdf2d_round_rect((in.local_px - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03);
        let refract_offset = refract(normalize((graph_inputs.node_GroupInstance_132_Vector3Input_80_2831bc4b).xyz), normalize(normalize(vec3f(-(((sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(1.0, 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03)) - (sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(-1.0, 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03))) * 0.5), -(((sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(0.0, 1.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03)) - (sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(0.0, -1.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03))) * 0.5), 1.0))), (1.0 / (1.450000048)));
        var output: vec2f;
        output = mc_GroupInstance_132_MathClosure_79_(in.uv, xy, size, depth, refract_offset);
        mc_GroupInstance_132_MathClosure_79_out = output;
    }
    var mc_GroupInstance_132_MathClosure_88_out: vec2f;
    {
        let uv = mc_GroupInstance_132_MathClosure_79_out;
        let scale = (graph_inputs.node_GroupInstance_132_FloatInput_89_b1b7ccd9).x;
        var output: vec2f;
        output = mc_GroupInstance_132_MathClosure_88_(in.uv, uv, scale);
        mc_GroupInstance_132_MathClosure_88_out = output;
    }
    var mc_GroupInstance_132_MathClosure_63_out: f32;
    {
        let n = normalize(vec3f(-(((sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(1.0, 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03)) - (sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(-1.0, 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03))) * 0.5), -(((sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(0.0, 1.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03)) - (sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(0.0, -1.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03))) * 0.5), 1.0));
        var output: f32;
        output = mc_GroupInstance_132_MathClosure_63_(in.uv, n);
        mc_GroupInstance_132_MathClosure_63_out = output;
    }
    var mc_GroupInstance_132_MathClosure_91_out: f32;
    {
        let x = mc_GroupInstance_132_MathClosure_63_out;
        var output: f32;
        output = mc_GroupInstance_132_MathClosure_91_(in.uv, x);
        mc_GroupInstance_132_MathClosure_91_out = output;
    }
    var mc_GroupInstance_132_MathClosure_104_out: f32;
    {
        let n = normalize(vec3f(-(((sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(1.0, 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03)) - (sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(-1.0, 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03))) * 0.5), -(((sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(0.0, 1.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03)) - (sdf2d_bevel_smooth5(sdf2d_round_rect(((in.local_px + vec2f(0.0, -1.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)), 24, 0.03))) * 0.5), 1.0));
        let i = (graph_inputs.node_GroupInstance_132_Vector3Input_105_b41cb78d).xyz;
        var output: f32;
        output = mc_GroupInstance_132_MathClosure_104_(in.uv, n, i);
        mc_GroupInstance_132_MathClosure_104_out = output;
    }
    var mc_GroupInstance_132_MathClosure_108_out: vec2f;
    {
        let t = (graph_inputs.node_FloatInput_137_f1da2f17).x;
        let size = in.geo_size_px;
        var output: vec2f;
        output = mc_GroupInstance_132_MathClosure_108_(in.uv, t, size);
        mc_GroupInstance_132_MathClosure_108_out = output;
    }
    var mc_GroupInstance_132_MathClosure_115_out: f32;
    {
        let sdf = (length((in.local_px - mc_GroupInstance_132_MathClosure_108_out)) - 16.5);
        let show_thumb = ((graph_inputs.node_BoolInput_140_13a147c1).x != 0);
        var output: f32;
        output = mc_GroupInstance_132_MathClosure_115_(in.uv, sdf, select(0.0, 1.0, show_thumb));
        mc_GroupInstance_132_MathClosure_115_out = output;
    }
    var mc_GroupInstance_132_MathClosure_96_out: vec4f;
    {
        let c_edge = textureSample(pass_tex_GroupInstance_132_GuassianBlurPass_85, pass_samp_GroupInstance_132_GuassianBlurPass_85, vec2f((mc_GroupInstance_132_MathClosure_88_out).x, 1.0 - (mc_GroupInstance_132_MathClosure_88_out).y));
        let e = smoothstep(0.0, -2.0, sdf2d_round_rect((in.local_px - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_132_FloatInput_10_d7a4e6d9).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_132_FloatInput_12_3da8e6d9).x)));
        let f = smoothstep(0.0, 0.015, mc_GroupInstance_132_MathClosure_91_out);
        let l = mc_GroupInstance_132_MathClosure_104_out;
        let selection = mc_GroupInstance_132_MathClosure_115_out;
        let lumin_edge = clamp(dot((textureSample(pass_tex_GroupInstance_132_GuassianBlurPass_85, pass_samp_GroupInstance_132_GuassianBlurPass_85, vec2f((mc_GroupInstance_132_MathClosure_88_out).x, 1.0 - (mc_GroupInstance_132_MathClosure_88_out).y))).rgb, vec3f(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
        var output: vec4f;
        output = mc_GroupInstance_132_MathClosure_96_(in.uv, c_edge, e, f, l, selection, lumin_edge);
        mc_GroupInstance_132_MathClosure_96_out = output;
    }
    var mc_GroupInstance_132_GroupInstance_125_MathClosure_99_out: vec2f;
    {
        let xy = in.local_px;
        let size = in.geo_size_px;
        var output: vec2f;
        output = mc_GroupInstance_132_GroupInstance_125_MathClosure_99_(in.uv, xy, size);
        mc_GroupInstance_132_GroupInstance_125_MathClosure_99_out = output;
    }
    var mc_GroupInstance_132_MathClosure_111_out: vec4f;
    {
        let t = smoothstep(0.0, 1.0, (length((in.local_px - mc_GroupInstance_132_MathClosure_108_out)) - 16.5));
        let c_ui = nf_premultiply(textureSample(img_tex_GroupInstance_132_ImageTexture_76, img_samp_GroupInstance_132_ImageTexture_76, (mc_GroupInstance_132_GroupInstance_125_MathClosure_99_out)));
        let thumb = smoothstep(-7.0, -8.0, (length((in.local_px - mc_GroupInstance_132_MathClosure_108_out)) - 16.5));
        let show_thumb = ((graph_inputs.node_BoolInput_140_13a147c1).x != 0);
        var output: vec4f;
        output = mc_GroupInstance_132_MathClosure_111_(in.uv, t, c_ui, thumb, select(0.0, 1.0, show_thumb));
        mc_GroupInstance_132_MathClosure_111_out = output;
    }
    return blendNormal((mc_GroupInstance_132_MathClosure_111_out), (mc_GroupInstance_132_MathClosure_96_out));
}
