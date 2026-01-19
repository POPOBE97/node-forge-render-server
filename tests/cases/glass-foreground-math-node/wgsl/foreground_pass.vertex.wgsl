
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

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
};

// --- Extra WGSL declarations (generated) ---
fn mc_math_artisticMapping(uv: vec2<f32>, combinedLight: f32, sdf: vec2<f32>, uStrength: f32) -> f32 {
    var uv_1: vec2<f32>;
    var combinedLight_1: f32;
    var sdf_1: vec2<f32>;
    var uStrength_1: f32;
    var output: f32 = 0f;
    var finalAlpha: f32;
    var nf: f32;
    var limit: f32;

    uv_1 = uv;
    combinedLight_1 = combinedLight;
    sdf_1 = sdf;
    uStrength_1 = uStrength;
    let _e11: f32 = combinedLight_1;
    let _e12: f32 = combinedLight_1;
    finalAlpha = (_e11 * _e12);
    let _e18: f32 = finalAlpha;
    finalAlpha = clamp(_e18, 0f, 1f);
    let _e22: vec2<f32> = sdf_1;
    let _e25: vec2<f32> = sdf_1;
    nf = max(_e25.y, 0.00001f);
    let _e31: vec2<f32> = sdf_1;
    let _e33: f32 = nf;
    limit = (1f + (_e31.x / _e33));
    let _e37: f32 = finalAlpha;
    let _e41: f32 = limit;
    let _e49: f32 = limit;
    finalAlpha = (_e37 * max((1f - pow(_e49, 8f)), 0f));
    let _e56: f32 = finalAlpha;
    let _e57: f32 = uStrength_1;
    finalAlpha = (_e56 * _e57);
    let _e59: f32 = finalAlpha;
    output = _e59;
    let _e60: f32 = output;
    return _e60;
}

fn mc_math_boxSdf(uv: vec2<f32>, p: vec4<f32>, b: vec4<f32>, r: vec3<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var p_1: vec4<f32>;
    var b_1: vec4<f32>;
    var r_1: vec3<f32>;
    var output: f32 = 0f;
    var radius: f32;
    var d: vec2<f32>;

    uv_1 = uv;
    p_1 = p;
    b_1 = b;
    r_1 = r;
    let _e11: vec3<f32> = r_1;
    radius = _e11.z;
    let _e15: vec4<f32> = p_1;
    let _e17: vec4<f32> = b_1;
    let _e19: f32 = radius;
    d = ((abs(_e15) - _e17) + vec4(_e19)).xy;
    let _e24: vec2<f32> = d;
    let _e26: vec2<f32> = d;
    let _e28: vec2<f32> = d;
    let _e30: vec2<f32> = d;
    let _e34: vec2<f32> = d;
    let _e36: vec2<f32> = d;
    let _e38: vec2<f32> = d;
    let _e40: vec2<f32> = d;
    let _e48: vec2<f32> = d;
    let _e55: vec2<f32> = d;
    let _e61: f32 = radius;
    output = ((min(max(_e38.x, _e40.y), 0f) + length(max(_e55, vec2(0f)))) - _e61);
    let _e63: f32 = output;
    return _e63;
}

fn mc_math_calcHalfSize(uv: vec2<f32>, uGeoPxSize: vec3<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var uGeoPxSize_1: vec3<f32>;
    var output: vec4<f32> = vec4(0f);
    var halfSizePx: vec2<f32>;
    var boxNormFactor: f32;
    var circleRadius: f32;

    uv_1 = uv;
    uGeoPxSize_1 = uGeoPxSize;
    let _e7: vec3<f32> = uGeoPxSize_1;
    halfSizePx = (_e7.xy * 0.5f);
    let _e12: vec2<f32> = halfSizePx;
    let _e14: vec2<f32> = halfSizePx;
    let _e16: vec2<f32> = halfSizePx;
    let _e18: vec2<f32> = halfSizePx;
    boxNormFactor = min(_e16.x, _e18.y);
    let _e22: f32 = boxNormFactor;
    circleRadius = _e22;
    let _e24: vec2<f32> = halfSizePx;
    let _e25: f32 = boxNormFactor;
    let _e26: f32 = circleRadius;
    output = vec4<f32>(_e24.x, _e24.y, _e25, _e26);
    let _e30: vec4<f32> = output;
    return _e30;
}

fn mc_math_calcLightCenter(uv: vec2<f32>, uLightCenter: vec2<f32>, uGeoPxSize: vec3<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var uLightCenter_1: vec2<f32>;
    var uGeoPxSize_1: vec3<f32>;
    var output: vec2<f32> = vec2(0f);
    var lightCenterPx: vec2<f32>;

    uv_1 = uv;
    uLightCenter_1 = uLightCenter;
    uGeoPxSize_1 = uGeoPxSize;
    let _e9: vec2<f32> = uLightCenter_1;
    let _e12: vec2<f32> = uLightCenter_1;
    let _e16: vec3<f32> = uGeoPxSize_1;
    lightCenterPx = (vec2<f32>(_e9.x, (1f - _e12.y)) * _e16.xy);
    let _e20: vec2<f32> = lightCenterPx;
    output = _e20;
    let _e21: vec2<f32> = output;
    return _e21;
}

fn mc_math_circleSdf(uv: vec2<f32>, p: vec4<f32>, r: vec4<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var p_1: vec4<f32>;
    var r_1: vec4<f32>;
    var output: f32 = 0f;
    var radius: f32;

    uv_1 = uv;
    p_1 = p;
    r_1 = r;
    let _e9: vec4<f32> = r_1;
    radius = _e9.w;
    let _e13: vec4<f32> = p_1;
    let _e15: f32 = radius;
    output = (length(_e13) - _e15);
    let _e17: f32 = output;
    return _e17;
}

fn mc_math_combineLights(uv: vec2<f32>, src1Light: f32, src2Light: f32, ring1Light: f32, ring2Light: f32) -> f32 {
    var uv_1: vec2<f32>;
    var src1Light_1: f32;
    var src2Light_1: f32;
    var ring1Light_1: f32;
    var ring2Light_1: f32;
    var output: f32 = 0f;

    uv_1 = uv;
    src1Light_1 = src1Light;
    src2Light_1 = src2Light;
    ring1Light_1 = ring1Light;
    ring2Light_1 = ring2Light;
    let _e13: f32 = src1Light_1;
    let _e14: f32 = src2Light_1;
    let _e16: f32 = ring1Light_1;
    let _e18: f32 = ring2Light_1;
    output = (((_e13 + _e14) + _e16) + _e18);
    let _e20: f32 = output;
    return _e20;
}

fn mc_math_finalColor(uv: vec2<f32>, uColor: vec4<f32>, finalAlpha: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var uColor_1: vec4<f32>;
    var finalAlpha_1: f32;
    var output: vec4<f32> = vec4(0f);
    var finalColor: vec4<f32>;

    uv_1 = uv;
    uColor_1 = uColor;
    finalAlpha_1 = finalAlpha;
    let _e9: vec4<f32> = uColor_1;
    finalColor = _e9;
    let _e11: vec4<f32> = finalColor;
    let _e12: f32 = finalAlpha_1;
    finalColor = (_e11 * _e12);
    let _e14: vec4<f32> = finalColor;
    output = _e14;
    let _e15: vec4<f32> = output;
    return _e15;
}

fn mc_math_posFromCenter(uv: vec2<f32>, uvPx: vec4<f32>, lightCenterPx: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var uvPx_1: vec4<f32>;
    var lightCenterPx_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);
    var uv_2: vec2<f32>;
    var centerPx: vec2<f32>;
    var posFromCenter: vec2<f32>;
    var posFromLightCenter: vec2<f32>;

    uv_1 = uv;
    uvPx_1 = uvPx;
    lightCenterPx_1 = lightCenterPx;
    let _e9: vec4<f32> = uvPx_1;
    uv_2 = _e9.xy;
    let _e12: vec4<f32> = uvPx_1;
    centerPx = _e12.zw;
    let _e15: vec2<f32> = uv_2;
    let _e16: vec2<f32> = centerPx;
    posFromCenter = (_e15 - _e16);
    let _e19: vec2<f32> = uv_2;
    let _e20: vec2<f32> = lightCenterPx_1;
    posFromLightCenter = (_e19 - _e20);
    let _e23: vec2<f32> = posFromCenter;
    let _e24: vec2<f32> = posFromLightCenter;
    output = vec4<f32>(_e23.x, _e23.y, _e24.x, _e24.y);
    let _e30: vec4<f32> = output;
    return _e30;
}

fn mc_math_ring1Light(uv: vec2<f32>, sdf: vec2<f32>, uRing1_: vec3<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var sdf_1: vec2<f32>;
    var uRing1_1: vec3<f32>;
    var output: f32 = 0f;
    var nf: f32;
    var lit: f32;

    uv_1 = uv;
    sdf_1 = sdf;
    uRing1_1 = uRing1_;
    let _e9: vec2<f32> = sdf_1;
    let _e12: vec2<f32> = sdf_1;
    nf = max(_e12.y, 0.00001f);
    let _e18: vec2<f32> = sdf_1;
    let _e20: f32 = nf;
    lit = (1f + (_e18.x / _e20));
    let _e27: f32 = lit;
    let _e28: vec3<f32> = uRing1_1;
    let _e31: vec3<f32> = uRing1_1;
    let _e34: f32 = lit;
    let _e35: vec3<f32> = uRing1_1;
    let _e38: vec3<f32> = uRing1_1;
    let _e44: f32 = lit;
    let _e45: vec3<f32> = uRing1_1;
    let _e48: vec3<f32> = uRing1_1;
    let _e51: f32 = lit;
    let _e52: vec3<f32> = uRing1_1;
    let _e55: vec3<f32> = uRing1_1;
    lit = (1f - smoothstep(0f, 1f, abs(((_e51 - _e52.x) * _e55.y))));
    let _e61: f32 = lit;
    let _e62: vec3<f32> = uRing1_1;
    output = (_e61 * _e62.z);
    let _e65: f32 = output;
    return _e65;
}

fn mc_math_ring2Light(uv: vec2<f32>, sdf: vec2<f32>, uRing2_: vec3<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var sdf_1: vec2<f32>;
    var uRing2_1: vec3<f32>;
    var output: f32 = 0f;
    var nf: f32;
    var lit: f32;

    uv_1 = uv;
    sdf_1 = sdf;
    uRing2_1 = uRing2_;
    let _e9: vec2<f32> = sdf_1;
    let _e12: vec2<f32> = sdf_1;
    nf = max(_e12.y, 0.00001f);
    let _e18: vec2<f32> = sdf_1;
    let _e20: f32 = nf;
    lit = (1f + (_e18.x / _e20));
    let _e27: f32 = lit;
    let _e28: vec3<f32> = uRing2_1;
    let _e31: vec3<f32> = uRing2_1;
    let _e34: f32 = lit;
    let _e35: vec3<f32> = uRing2_1;
    let _e38: vec3<f32> = uRing2_1;
    let _e44: f32 = lit;
    let _e45: vec3<f32> = uRing2_1;
    let _e48: vec3<f32> = uRing2_1;
    let _e51: f32 = lit;
    let _e52: vec3<f32> = uRing2_1;
    let _e55: vec3<f32> = uRing2_1;
    lit = (1f - smoothstep(0f, 1f, abs(((_e51 - _e52.x) * _e55.y))));
    let _e61: f32 = lit;
    let _e62: vec3<f32> = uRing2_1;
    output = (_e61 * _e62.z);
    let _e65: f32 = output;
    return _e65;
}

fn mc_math_sdfMorph(uv: vec2<f32>, circleSdf: f32, boxSdf: f32, circleNormFactor: vec4<f32>, boxNormFactor: vec4<f32>, uShapeMorph: f32) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var circleSdf_1: f32;
    var boxSdf_1: f32;
    var circleNormFactor_1: vec4<f32>;
    var boxNormFactor_1: vec4<f32>;
    var uShapeMorph_1: f32;
    var output: vec2<f32> = vec2(0f);
    var circleNF: f32;
    var boxNF: f32;
    var sdf: f32;
    var normFactor: f32;

    uv_1 = uv;
    circleSdf_1 = circleSdf;
    boxSdf_1 = boxSdf;
    circleNormFactor_1 = circleNormFactor;
    boxNormFactor_1 = boxNormFactor;
    uShapeMorph_1 = uShapeMorph;
    let _e15: vec4<f32> = circleNormFactor_1;
    circleNF = _e15.w;
    let _e18: vec4<f32> = boxNormFactor_1;
    boxNF = _e18.z;
    let _e24: f32 = circleSdf_1;
    let _e25: f32 = boxSdf_1;
    let _e26: f32 = uShapeMorph_1;
    sdf = mix(_e24, _e25, _e26);
    let _e32: f32 = circleNF;
    let _e33: f32 = boxNF;
    let _e34: f32 = uShapeMorph_1;
    normFactor = mix(_e32, _e33, _e34);
    let _e37: f32 = sdf;
    let _e38: f32 = normFactor;
    output = vec2<f32>(_e37, _e38);
    let _e40: vec2<f32> = output;
    return _e40;
}

fn mc_math_src1Light(uv: vec2<f32>, sdf: vec2<f32>, uSrc1_: vec2<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var sdf_1: vec2<f32>;
    var uSrc1_1: vec2<f32>;
    var output: f32 = 0f;
    var nf: f32;
    var lit: f32;

    uv_1 = uv;
    sdf_1 = sdf;
    uSrc1_1 = uSrc1_;
    let _e9: vec2<f32> = sdf_1;
    let _e12: vec2<f32> = sdf_1;
    nf = max(_e12.y, 0.00001f);
    let _e18: vec2<f32> = sdf_1;
    let _e20: f32 = nf;
    lit = (1f + (_e18.x / _e20));
    let _e24: f32 = lit;
    let _e26: f32 = lit;
    let _e28: vec2<f32> = uSrc1_1;
    let _e31: f32 = lit;
    let _e33: f32 = lit;
    let _e35: vec2<f32> = uSrc1_1;
    lit = exp(((-(_e31) * _e33) * _e35.x));
    let _e39: f32 = lit;
    let _e40: vec2<f32> = uSrc1_1;
    output = (_e39 * _e40.y);
    let _e43: f32 = output;
    return _e43;
}

fn mc_math_src2Light(uv: vec2<f32>, sdf: vec2<f32>, uSrc2_: vec2<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var sdf_1: vec2<f32>;
    var uSrc2_1: vec2<f32>;
    var output: f32 = 0f;
    var nf: f32;
    var lit: f32;

    uv_1 = uv;
    sdf_1 = sdf;
    uSrc2_1 = uSrc2_;
    let _e9: vec2<f32> = sdf_1;
    let _e12: vec2<f32> = sdf_1;
    nf = max(_e12.y, 0.00001f);
    let _e18: vec2<f32> = sdf_1;
    let _e20: f32 = nf;
    lit = (1f + (_e18.x / _e20));
    let _e26: f32 = lit;
    let _e27: vec2<f32> = uSrc2_1;
    let _e33: f32 = lit;
    let _e34: vec2<f32> = uSrc2_1;
    let _e42: f32 = lit;
    let _e43: vec2<f32> = uSrc2_1;
    let _e49: f32 = lit;
    let _e50: vec2<f32> = uSrc2_1;
    lit = (1f - max(pow((1f - (_e49 * _e50.x)), 3f), 0f));
    let _e59: f32 = lit;
    let _e60: vec2<f32> = uSrc2_1;
    output = (_e59 * _e60.y);
    let _e63: f32 = output;
    return _e63;
}

fn mc_math_uvToPixel(uv: vec2<f32>, vUv: vec2<f32>, uGeoPxSize: vec3<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var vUv_1: vec2<f32>;
    var uGeoPxSize_1: vec3<f32>;
    var output: vec4<f32> = vec4(0f);
    var screenUV: vec2<f32>;
    var uvPx: vec2<f32>;
    var centerPx: vec2<f32>;

    uv_1 = uv;
    vUv_1 = vUv;
    uGeoPxSize_1 = uGeoPxSize;
    let _e9: vec2<f32> = uv_1;
    screenUV = _e9;
    let _e11: vec2<f32> = screenUV;
    let _e12: vec3<f32> = uGeoPxSize_1;
    uvPx = (_e11 * _e12.xy);
    let _e16: vec3<f32> = uGeoPxSize_1;
    centerPx = (_e16.xy * 0.5f);
    let _e22: vec3<f32> = uGeoPxSize_1;
    let _e24: vec2<f32> = centerPx;
    centerPx.y = (_e22.y - _e24.y);
    let _e27: vec2<f32> = uvPx;
    let _e28: vec2<f32> = centerPx;
    output = vec4<f32>(_e27.x, _e27.y, _e28.x, _e28.y);
    let _e34: vec4<f32> = output;
    return _e34;
}


 @vertex
 fn vs_main(@location(0) position: vec3f) -> VSOut {
 var out: VSOut;

 // Local UV in [0,1] based on geometry size.
 out.uv = (position.xy / params.geo_size) + vec2f(0.5, 0.5);

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 let p_px = params.center + position.xy + (params.target_size * 0.5);

 // Convert pixels to clip space assuming bottom-left origin.
 // (0,0) => (-1,-1), (target_size) => (1,1)
 let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
 out.position = vec4f(ndc, position.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
 