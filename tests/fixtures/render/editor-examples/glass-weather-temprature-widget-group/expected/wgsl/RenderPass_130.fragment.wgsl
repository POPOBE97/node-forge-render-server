
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
    // Node: BoolInput_141
    node_BoolInput_141_609f47c1: vec4i,
    // Node: FloatInput_138
    node_FloatInput_138_0ccf2f17: vec4f,
    // Node: GroupInstance_135/FloatInput_10
    node_GroupInstance_135_FloatInput_10_2a8c3aed: vec4f,
    // Node: GroupInstance_135/FloatInput_12
    node_GroupInstance_135_FloatInput_12_c4883aed: vec4f,
    // Node: GroupInstance_135/FloatInput_89
    node_GroupInstance_135_FloatInput_89_00da25ed: vec4f,
    // Node: GroupInstance_135/Vector3Input_105
    node_GroupInstance_135_Vector3Input_105_1bad8802: vec4f,
    // Node: GroupInstance_135/Vector3Input_80
    node_GroupInstance_135_Vector3Input_80_cd4f4a91: vec4f,
    // Node: GroupInstance_135/dp.Sdf2DBevel_61.width
    node_GroupInstance_135_dp_Sdf2DBevel_61_width_c19922df: vec4f,
    // Node: GroupInstance_135/dp.Sdf2D_106.radius
    node_GroupInstance_135_dp_Sdf2D_106_radius_db8e8892: vec4f,
    // Node: GroupInstance_135/dp.metric.MathClosure_115.falloffPx
    node_GroupInstance_135_dp_metric_MathClosure_115_falloffPx_a1c290b5: vec4f,
    // Node: GroupInstance_135/dp.metric.MathClosure_115.radiusPx
    node_GroupInstance_135_dp_metric_MathClosure_115_radiusPx_d72bcff5: vec4f,
    // Node: GroupInstance_135/dp.metric.Remap_112.edge1
    node_GroupInstance_135_dp_metric_Remap_112_edge1_94b6734d: vec4f,
    // Node: GroupInstance_135/dp.metric.Remap_113.edge0
    node_GroupInstance_135_dp_metric_Remap_113_edge0_94217550: vec4f,
    // Node: GroupInstance_135/dp.metric.Remap_113.edge1
    node_GroupInstance_135_dp_metric_Remap_113_edge1_47237550: vec4f,
    // Node: GroupInstance_135/dp.metric.Remap_64.edge1
    node_GroupInstance_135_dp_metric_Remap_64_edge1_f6357d43: vec4f,
    // Node: Vector2Input_144
    node_Vector2Input_144_b8cd7189: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_GroupInstance_135_ImageTexture_76: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_GroupInstance_135_ImageTexture_76: sampler;

@group(1) @binding(2)
var pass_tex_GroupInstance_135_PassTexture_86: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_GroupInstance_135_PassTexture_86: sampler;


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

fn nf_math_closure_fragment_0f7f3acb_closure(uv: vec2<f32>, n: vec3<f32>, i: vec3<f32>) -> f32 {
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

fn nf_math_closure_fragment_1b35870a_closure(uv: vec2<f32>, n: vec3<f32>) -> f32 {
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

fn nf_math_closure_fragment_28d85e0a_closure(uv: vec2<f32>, sampleUV: vec2<f32>, scale: f32) -> vec2<f32> {
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

fn nf_math_closure_fragment_45d5ce8f_closure(uv_2: vec2<f32>, sampleUV: vec2<f32>) -> vec4<f32> {
    var uv_3: vec2<f32>;
    var sampleUV_1: vec2<f32>;
    var output: vec4<f32>;

    uv_3 = uv_2;
    sampleUV_1 = sampleUV;
    let _e6: vec2<f32> = sampleUV_1;
    let _e7: vec4<f32> = nf_math_closure_fragment_45d5ce8f_closure_sample_texture_texture(_e6);
    output = _e7;
    let _e8: vec4<f32> = output;
    return _e8;
}

fn nf_math_closure_fragment_45d5ce8f_closure_sample_texture_texture(uv: vec2f) -> vec4f {
    let raster_uv = vec2f(uv.x, 1.0 - uv.y);
    return textureSample(pass_tex_GroupInstance_135_PassTexture_86, pass_samp_GroupInstance_135_PassTexture_86, raster_uv);
}

fn nf_math_closure_fragment_8c67890a_closure(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>, depth: f32, refract_offset: vec3<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var depth_1: f32;
    var refract_offset_1: vec3<f32>;
    var output: vec2<f32>;
    var offset: vec2<f32>;

    uv_1 = uv;
    xy_1 = xy;
    size_1 = size;
    depth_1 = depth;
    refract_offset_1 = refract_offset;
    let _e11: vec2<f32> = xy_1;
    let _e12: vec3<f32> = refract_offset_1;
    let _e14: f32 = depth_1;
    offset = (_e11 + (_e12.xy * _e14));
    let _e18: vec2<f32> = offset;
    let _e20: vec2<f32> = size_1;
    output = (_e18.xy / _e20);
    let _e22: vec2<f32> = output;
    return _e22;
}

fn nf_math_closure_fragment_closure(uv: vec2<f32>, c_edge: vec4<f32>, e: f32, f: f32, l: f32, selection: f32, lumin_edge: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var c_edge_1: vec4<f32>;
    var e_1: f32;
    var f_1: f32;
    var l_1: f32;
    var selection_1: f32;
    var lumin_edge_1: f32;
    var output: vec4<f32>;

    uv_1 = uv;
    c_edge_1 = c_edge;
    e_1 = e;
    f_1 = f;
    l_1 = l;
    selection_1 = selection;
    lumin_edge_1 = lumin_edge;
    let _e15: vec4<f32> = c_edge_1;
    let _e19: vec4<f32> = c_edge_1;
    let _e24: vec4<f32> = c_edge_1;
    let _e26: f32 = lumin_edge_1;
    let _e28: vec3<f32> = mix(vec3(1f), _e24.xyz, vec3(_e26));
    c_edge_1.x = _e28.x;
    c_edge_1.y = _e28.y;
    c_edge_1.z = _e28.z;
    let _e36: vec4<f32> = c_edge_1;
    let _e38: f32 = e_1;
    let _e39: f32 = f_1;
    c_edge_1.w = (_e36.w * ((_e38 * _e39) * 0.05f));
    let _e45: vec4<f32> = c_edge_1;
    let _e52: f32 = lumin_edge_1;
    let _e54: f32 = l_1;
    let _e55: f32 = f_1;
    let _e60: f32 = selection_1;
    let _e61: f32 = f_1;
    let _e66: f32 = e_1;
    c_edge_1.w = (_e45.w + (((mix(0.08f, 0.22f, _e52) + ((_e54 * _e55) * 0.6f)) + ((_e60 * _e61) * 0.2f)) * _e66));
    let _e69: vec4<f32> = c_edge_1;
    let _e71: vec4<f32> = c_edge_1;
    let _e75: f32 = l_1;
    let _e76: f32 = f_1;
    let _e78: f32 = selection_1;
    let _e79: f32 = f_1;
    let _e82: vec4<f32> = c_edge_1;
    let _e86: f32 = l_1;
    let _e87: f32 = f_1;
    let _e89: f32 = selection_1;
    let _e90: f32 = f_1;
    let _e94: vec3<f32> = mix(_e82.xyz, vec3(1f), vec3(((_e86 * _e87) + (_e89 * _e90))));
    c_edge_1.x = _e94.x;
    c_edge_1.y = _e94.y;
    c_edge_1.z = _e94.z;
    let _e101: vec4<f32> = c_edge_1;
    let _e103: vec4<f32> = c_edge_1;
    let _e105: vec4<f32> = c_edge_1;
    let _e107: vec3<f32> = (_e103.xyz * _e105.w);
    c_edge_1.x = _e107.x;
    c_edge_1.y = _e107.y;
    c_edge_1.z = _e107.z;
    let _e114: vec4<f32> = c_edge_1;
    output = _e114;
    let _e115: vec4<f32> = output;
    return _e115;
}

fn nf_math_closure_fragment_e608610a_closure(uv: vec2<f32>, x: f32) -> f32 {
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

fn nf_show_thumb_fragment_cd9637cb_closure(uv: vec2<f32>, t: f32, c_ui: vec4<f32>, thumb: f32, show_thumb: bool) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var t_1: f32;
    var c_ui_1: vec4<f32>;
    var thumb_1: f32;
    var show_thumb_1: bool;
    var output: vec4<f32>;
    var r: vec4<f32>;

    uv_1 = uv;
    t_1 = t;
    c_ui_1 = c_ui;
    thumb_1 = thumb;
    show_thumb_1 = show_thumb;
    let _e11: vec4<f32> = c_ui_1;
    let _e16: f32 = t_1;
    let _e17: bool = show_thumb_1;
    r = (_e11 * select(1f, _e16, _e17));
    let _e24: f32 = thumb_1;
    let _e25: bool = show_thumb_1;
    let _e30: vec4<f32> = r;
    let _e33: f32 = thumb_1;
    let _e34: bool = show_thumb_1;
    r = mix(_e30, vec4(1f), vec4((_e33 * select(0f, 1f, _e34))));
    let _e41: vec4<f32> = r;
    output = _e41;
    let _e42: vec4<f32> = output;
    return _e42;
}

fn nf_show_thumb_fragment_closure(uv: vec2<f32>, sdf: f32, show_thumb: bool, radiusPx: f32, falloffPx: f32) -> f32 {
    var uv_1: vec2<f32>;
    var sdf_1: f32;
    var show_thumb_1: bool;
    var radiusPx_1: f32;
    var falloffPx_1: f32;
    var output: f32;
    var r: f32;

    uv_1 = uv;
    sdf_1 = sdf;
    show_thumb_1 = show_thumb;
    radiusPx_1 = radiusPx;
    falloffPx_1 = falloffPx;
    let _e11: f32 = sdf_1;
    let _e12: f32 = radiusPx_1;
    let _e15: f32 = sdf_1;
    let _e16: f32 = radiusPx_1;
    r = max((_e15 + _e16), 0f);
    let _e21: f32 = r;
    let _e22: f32 = falloffPx_1;
    r = (_e21 / _e22);
    let _e24: f32 = r;
    let _e26: f32 = r;
    let _e28: f32 = r;
    let _e30: f32 = r;
    let _e34: bool = show_thumb_1;
    output = (f32(exp((-(_e28) * _e30))) * select(0f, 1f, _e34));
    let _e39: f32 = output;
    return _e39;
}

fn nf_thumb_t_fragment_closure(uv: vec2<f32>, t: f32, size: vec2<f32>) -> vec2<f32> {
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


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Sdf2DBevel GroupInstance_135/Sdf2DBevel_61.depth
    let _2d_sdf_bevel_depth_sdf_depth = sdf2d_round_rect(
        (in.local_px.xy - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_135_FloatInput_10_2a8c3aed).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.node_GroupInstance_135_FloatInput_12_c4883aed).x),
    );
    let _2d_sdf_bevel_depth_depth = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_depth_sdf_depth,
        (graph_inputs.node_GroupInstance_135_dp_Sdf2DBevel_61_width_c19922df).x,
        0.03,
    );
    // Sdf2DBevel GroupInstance_135/Sdf2DBevel_61.normal finite differences
    let _2d_sdf_bevel_normal_sdf_px = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(sdf2d_bevel_eps(), 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_135_FloatInput_10_2a8c3aed).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.node_GroupInstance_135_FloatInput_12_c4883aed).x),
    );
    let _2d_sdf_bevel_normal_sdf_nx = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(-(sdf2d_bevel_eps()), 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_135_FloatInput_10_2a8c3aed).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.node_GroupInstance_135_FloatInput_12_c4883aed).x),
    );
    let _2d_sdf_bevel_normal_sdf_py = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(0.0, sdf2d_bevel_eps())) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_135_FloatInput_10_2a8c3aed).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.node_GroupInstance_135_FloatInput_12_c4883aed).x),
    );
    let _2d_sdf_bevel_normal_sdf_ny = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(0.0, -(sdf2d_bevel_eps()))) - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_135_FloatInput_10_2a8c3aed).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.node_GroupInstance_135_FloatInput_12_c4883aed).x),
    );
    let _2d_sdf_bevel_normal_depth_px = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_normal_sdf_px,
        (graph_inputs.node_GroupInstance_135_dp_Sdf2DBevel_61_width_c19922df).x,
        0.03,
    );
    let _2d_sdf_bevel_normal_depth_nx = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_normal_sdf_nx,
        (graph_inputs.node_GroupInstance_135_dp_Sdf2DBevel_61_width_c19922df).x,
        0.03,
    );
    let _2d_sdf_bevel_normal_depth_py = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_normal_sdf_py,
        (graph_inputs.node_GroupInstance_135_dp_Sdf2DBevel_61_width_c19922df).x,
        0.03,
    );
    let _2d_sdf_bevel_normal_depth_ny = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_normal_sdf_ny,
        (graph_inputs.node_GroupInstance_135_dp_Sdf2DBevel_61_width_c19922df).x,
        0.03,
    );
    let _2d_sdf_bevel_normal_normal = sdf2d_bevel_normal(
        _2d_sdf_bevel_normal_depth_px,
        _2d_sdf_bevel_normal_depth_nx,
        _2d_sdf_bevel_normal_depth_py,
        _2d_sdf_bevel_normal_depth_ny,
        sdf2d_bevel_eps(),
    );
    let math_closure_out = nf_math_closure_fragment_8c67890a_closure(vec2f(in.uv.x, 1.0 - in.uv.y), in.local_px.xy, in.geo_size_px, _2d_sdf_bevel_depth_depth, refract(normalize((graph_inputs.node_GroupInstance_135_Vector3Input_80_cd4f4a91).xyz), normalize(_2d_sdf_bevel_normal_normal), (1.0 / (1.450000048))));
    let math_closure_out_19d77a56 = nf_math_closure_fragment_28d85e0a_closure(vec2f(in.uv.x, 1.0 - in.uv.y), math_closure_out, (graph_inputs.node_GroupInstance_135_FloatInput_89_00da25ed).x);
    let math_closure_out_0e16ccf8 = nf_math_closure_fragment_45d5ce8f_closure(vec2f(in.uv.x, 1.0 - in.uv.y), math_closure_out_19d77a56);
    // Remap GroupInstance_135/Remap_64.result
    let remap = smoothstep(
        0.0,
        (graph_inputs.node_GroupInstance_135_dp_metric_Remap_64_edge1_f6357d43).x,
        sdf2d_round_rect((in.local_px.xy - (in.geo_size_px * vec2f((graph_inputs.node_GroupInstance_135_FloatInput_10_2a8c3aed).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_GroupInstance_135_FloatInput_12_c4883aed).x)),
    );
    let math_closure_out_c8fa2bf0 = nf_math_closure_fragment_1b35870a_closure(vec2f(in.uv.x, 1.0 - in.uv.y), _2d_sdf_bevel_normal_normal);
    let math_closure_out_abfa5bd4 = nf_math_closure_fragment_e608610a_closure(vec2f(in.uv.x, 1.0 - in.uv.y), math_closure_out_c8fa2bf0);
    let math_closure_out_6456fe79 = nf_math_closure_fragment_0f7f3acb_closure(vec2f(in.uv.x, 1.0 - in.uv.y), _2d_sdf_bevel_normal_normal, (graph_inputs.node_GroupInstance_135_Vector3Input_105_1bad8802).xyz);
    let thumb_t_out = nf_thumb_t_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), (graph_inputs.node_FloatInput_138_0ccf2f17).x, in.geo_size_px);
    let show_thumb_out = nf_show_thumb_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), (length((in.local_px.xy - thumb_t_out)) - (graph_inputs.node_GroupInstance_135_dp_Sdf2D_106_radius_db8e8892).x), ((graph_inputs.node_BoolInput_141_609f47c1).x != 0), (graph_inputs.node_GroupInstance_135_dp_metric_MathClosure_115_radiusPx_d72bcff5).x, (graph_inputs.node_GroupInstance_135_dp_metric_MathClosure_115_falloffPx_a1c290b5).x);
    let math_closure_out_149f3455 = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), math_closure_out_0e16ccf8, remap, smoothstep(0.0, 0.015, math_closure_out_abfa5bd4), math_closure_out_6456fe79, show_thumb_out, clamp(dot((math_closure_out_0e16ccf8).rgb, vec3f(0.2126, 0.7152, 0.0722)), 0.0, 1.0));
    // Remap GroupInstance_135/Remap_112.result
    let remap_b7aae9bc = smoothstep(
        0.0,
        (graph_inputs.node_GroupInstance_135_dp_metric_Remap_112_edge1_94b6734d).x,
        (length((in.local_px.xy - thumb_t_out)) - (graph_inputs.node_GroupInstance_135_dp_Sdf2D_106_radius_db8e8892).x),
    );
    // ImageTexture GroupInstance_135/ImageTexture_76 aspect-correct uv
    let image_uv = aspect_correct_uv_fill(
        (in.uv),
        vec2f(textureDimensions(img_tex_GroupInstance_135_ImageTexture_76)),
        in.geo_size_px,
    );
    // ImageTexture GroupInstance_135/ImageTexture_76.color
    let image_sample = textureSample(
        img_tex_GroupInstance_135_ImageTexture_76,
        img_samp_GroupInstance_135_ImageTexture_76,
        image_uv,
    );
    // Remap GroupInstance_135/Remap_113.result
    let remap_dc128ee9 = smoothstep(
        (graph_inputs.node_GroupInstance_135_dp_metric_Remap_113_edge0_94217550).x,
        (graph_inputs.node_GroupInstance_135_dp_metric_Remap_113_edge1_47237550).x,
        (length((in.local_px.xy - thumb_t_out)) - (graph_inputs.node_GroupInstance_135_dp_Sdf2D_106_radius_db8e8892).x),
    );
    let show_thumb_out_b69a1bc0 = nf_show_thumb_fragment_cd9637cb_closure(vec2f(in.uv.x, 1.0 - in.uv.y), remap_b7aae9bc, image_sample, remap_dc128ee9, ((graph_inputs.node_BoolInput_141_609f47c1).x != 0));
    // Final composite
    let _frag_out = blendNormal((show_thumb_out_b69a1bc0), (math_closure_out_149f3455));
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
