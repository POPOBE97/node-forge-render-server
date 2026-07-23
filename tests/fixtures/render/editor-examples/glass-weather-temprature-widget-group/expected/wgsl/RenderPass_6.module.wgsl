
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
  };


struct GraphInputs {
    // Node: BoolInput_139
    show_thumb: vec4i,
    // Node: FloatInput_136
    thumb_pos: vec4f,
    // Node: GroupInstance_128/FloatInput_10
    group_instance_128_float_input_10: vec4f,
    // Node: GroupInstance_128/FloatInput_12
    group_instance_128_float_input_12: vec4f,
    // Node: GroupInstance_128/FloatInput_89
    group_instance_128_float_input_89: vec4f,
    // Node: GroupInstance_128/Vector3Input_105
    group_instance_128_vector3_input_105: vec4f,
    // Node: GroupInstance_128/Vector3Input_80
    group_instance_128_vector3_input_80: vec4f,
    // Node: Vector2Input_142
    node_Vector2Input_142_ead77189: vec4f,
    // Node: Vector2Input_145
    node_Vector2Input_145_6bcf7189: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

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

fn mc_edge_color(uv: vec2<f32>, c_edge: vec4<f32>, e: f32, f: f32, l: f32, selection: f32, lumin_edge: f32) -> vec4<f32> {
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

fn mc_edge_highlight(uv: vec2<f32>, sdf: f32, show_thumb: f32) -> f32 {
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

fn mc_math_closure(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>, depth: f32, refract_offset: vec3<f32>) -> vec2<f32> {
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

fn mc_math_closure_3c18d25c(uv: vec2<f32>, x: f32) -> f32 {
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

fn mc_math_closure_9138d55c(uv: vec2<f32>, n: vec3<f32>) -> f32 {
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

fn mc_math_closure_b9b5e5df(uv: vec2<f32>, n: vec3<f32>, i: vec3<f32>) -> f32 {
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

fn mc_math_closure_fe0dcf5c(uv: vec2<f32>, uv_1: vec2<f32>, scale: f32) -> vec2<f32> {
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

fn mc_show_thumb(uv: vec2<f32>, t: f32, c_ui: vec4<f32>, thumb: f32, show_thumb: f32) -> vec4<f32> {
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

fn mc_thumb_t(uv: vec2<f32>, t: f32, size: vec2<f32>) -> vec2<f32> {
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


// ---- 2D SDF bevel helpers (generated) ----
// 2D SDF bevel helper template.
//
// This file is the editable WGSL source for Sdf2DBevel curve helper functions.
// The Rust compiler wires node inputs into calls to these helpers.

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
            x = sdf2d_bevel_smooth5_map(pow(x, 0.5));
            x = 1.0 - pow(1.0 - x, cliff);
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
 ) -> VSOut {
 var out: VSOut;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = uv;

 let rect_size_px_base = (graph_inputs.node_Vector2Input_142_ead77189).xy;
 let rect_center_px = (graph_inputs.node_Vector2Input_145_6bcf7189).xy;
 let rect_dyn = vec4f(rect_center_px, rect_size_px_base);
 out.geo_size_px = rect_dyn.zw;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, 0.0);

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
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Sdf2DBevel GroupInstance_128/Sdf2DBevel_61.depth
    let _2d_sdf_bevel_depth_sdf_depth = sdf2d_round_rect(
        (in.local_px.xy - (in.geo_size_px * vec2f((graph_inputs.group_instance_128_float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.group_instance_128_float_input_12).x),
    );
    let _2d_sdf_bevel_depth_depth = sdf2d_bevel_smooth5(_2d_sdf_bevel_depth_sdf_depth, 24, 0.03);
    // Sdf2DBevel GroupInstance_128/Sdf2DBevel_61.normal finite differences
    let _2d_sdf_bevel_normal_sdf_px = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(sdf2d_bevel_eps(), 0.0)) - (in.geo_size_px * vec2f((graph_inputs.group_instance_128_float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.group_instance_128_float_input_12).x),
    );
    let _2d_sdf_bevel_normal_sdf_nx = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(-(sdf2d_bevel_eps()), 0.0)) - (in.geo_size_px * vec2f((graph_inputs.group_instance_128_float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.group_instance_128_float_input_12).x),
    );
    let _2d_sdf_bevel_normal_sdf_py = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(0.0, sdf2d_bevel_eps())) - (in.geo_size_px * vec2f((graph_inputs.group_instance_128_float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.group_instance_128_float_input_12).x),
    );
    let _2d_sdf_bevel_normal_sdf_ny = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(0.0, -(sdf2d_bevel_eps()))) - (in.geo_size_px * vec2f((graph_inputs.group_instance_128_float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.group_instance_128_float_input_12).x),
    );
    let _2d_sdf_bevel_normal_depth_px = sdf2d_bevel_smooth5(_2d_sdf_bevel_normal_sdf_px, 24, 0.03);
    let _2d_sdf_bevel_normal_depth_nx = sdf2d_bevel_smooth5(_2d_sdf_bevel_normal_sdf_nx, 24, 0.03);
    let _2d_sdf_bevel_normal_depth_py = sdf2d_bevel_smooth5(_2d_sdf_bevel_normal_sdf_py, 24, 0.03);
    let _2d_sdf_bevel_normal_depth_ny = sdf2d_bevel_smooth5(_2d_sdf_bevel_normal_sdf_ny, 24, 0.03);
    let _2d_sdf_bevel_normal_normal = sdf2d_bevel_normal(
        _2d_sdf_bevel_normal_depth_px,
        _2d_sdf_bevel_normal_depth_nx,
        _2d_sdf_bevel_normal_depth_py,
        _2d_sdf_bevel_normal_depth_ny,
        sdf2d_bevel_eps(),
    );
    var math_closure_out_dc2d0740: vec2f;
    {
        let xy = in.local_px.xy;
        let size = in.geo_size_px;
        let depth = _2d_sdf_bevel_depth_depth;
        let refract_offset = refract(normalize((graph_inputs.group_instance_128_vector3_input_80).xyz), normalize(_2d_sdf_bevel_normal_normal), (1.0 / (1.450000048)));
        var output: vec2f;
        output = mc_math_closure(in.uv, xy, size, depth, refract_offset);
        math_closure_out_dc2d0740 = output;
    }
    var math_closure_out: vec2f;
    {
        let uv = math_closure_out_dc2d0740;
        let scale = (graph_inputs.group_instance_128_float_input_89).x;
        var output: vec2f;
        output = mc_math_closure_fe0dcf5c(in.uv, uv, scale);
        math_closure_out = output;
    }
    // Pass Texture GroupInstance_128/PassTexture_86.color
    let pass_texture = textureSample(
        pass_tex_GroupInstance_128_GuassianBlurPass_85,
        pass_samp_GroupInstance_128_GuassianBlurPass_85,
        vec2f((math_closure_out).x, 1.0 - (math_closure_out).y),
    );
    // Remap GroupInstance_128/Remap_64.result
    let remap = smoothstep(
        0.0,
        -2.0,
        sdf2d_round_rect((in.local_px.xy - (in.geo_size_px * vec2f((graph_inputs.group_instance_128_float_input_10).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.group_instance_128_float_input_12).x)),
    );
    var math_closure_out_a547f027: f32;
    {
        let n = _2d_sdf_bevel_normal_normal;
        var output: f32;
        output = mc_math_closure_9138d55c(in.uv, n);
        math_closure_out_a547f027 = output;
    }
    var math_closure_out_fe28cf44: f32;
    {
        let x = math_closure_out_a547f027;
        var output: f32;
        output = mc_math_closure_3c18d25c(in.uv, x);
        math_closure_out_fe28cf44 = output;
    }
    var math_closure_out_77f2a4b0: f32;
    {
        let n = _2d_sdf_bevel_normal_normal;
        let i = (graph_inputs.group_instance_128_vector3_input_105).xyz;
        var output: f32;
        output = mc_math_closure_b9b5e5df(in.uv, n, i);
        math_closure_out_77f2a4b0 = output;
    }
    var thumb_t_out: vec2f;
    {
        let t = (graph_inputs.thumb_pos).x;
        let size = in.geo_size_px;
        var output: vec2f;
        output = mc_thumb_t(in.uv, t, size);
        thumb_t_out = output;
    }
    var edge_highlight_out: f32;
    {
        let sdf = (length((in.local_px.xy - thumb_t_out)) - 16.5);
        let show_thumb = ((graph_inputs.show_thumb).x != 0);
        var output: f32;
        output = mc_edge_highlight(in.uv, sdf, select(0.0, 1.0, show_thumb));
        edge_highlight_out = output;
    }
    var edge_color_out: vec4f;
    {
        let c_edge = pass_texture;
        let e = remap;
        let f = smoothstep(0.0, 0.015, math_closure_out_fe28cf44);
        let l = math_closure_out_77f2a4b0;
        let selection = edge_highlight_out;
        let lumin_edge = clamp(dot((pass_texture).rgb, vec3f(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
        var output: vec4f;
        output = mc_edge_color(in.uv, c_edge, e, f, l, selection, lumin_edge);
        edge_color_out = output;
    }
    // ImageTexture GroupInstance_128/ImageTexture_76 aspect-correct uv
    let image_uv = aspect_correct_uv_fill(
        (in.uv),
        vec2f(textureDimensions(img_tex_GroupInstance_128_ImageTexture_76)),
        in.geo_size_px,
    );
    // ImageTexture GroupInstance_128/ImageTexture_76.color
    let image_sample = textureSample(
        img_tex_GroupInstance_128_ImageTexture_76,
        img_samp_GroupInstance_128_ImageTexture_76,
        image_uv,
    );
    var show_thumb_out: vec4f;
    {
        let t = smoothstep(0.0, 1.0, (length((in.local_px.xy - thumb_t_out)) - 16.5));
        let c_ui = image_sample;
        let thumb = smoothstep(-7.0, -8.0, (length((in.local_px.xy - thumb_t_out)) - 16.5));
        let show_thumb = ((graph_inputs.show_thumb).x != 0);
        var output: vec4f;
        output = mc_show_thumb(in.uv, t, c_ui, thumb, select(0.0, 1.0, show_thumb));
        show_thumb_out = output;
    }
    // Final composite
    let _frag_out = blendNormal((show_thumb_out), (edge_color_out));
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
