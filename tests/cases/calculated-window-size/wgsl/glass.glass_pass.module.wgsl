
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,
    time: f32,
    _pad0: f32,
    color: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};

// --- Extra WGSL declarations (generated) ---
fn mc_math_boxSdf(uv: vec2<f32>, uvPx: vec2<f32>, centerPx: vec2<f32>, uGeoPxSize: vec3<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var uvPx_1: vec2<f32>;
    var centerPx_1: vec2<f32>;
    var uGeoPxSize_1: vec3<f32>;
    var output: f32 = 0f;
    var p: vec2<f32>;
    var b: vec2<f32>;
    var r: f32;
    var d: vec2<f32>;
    var sdf: f32;

    uv_1 = uv;
    uvPx_1 = uvPx;
    centerPx_1 = centerPx;
    uGeoPxSize_1 = uGeoPxSize;
    let _e11: vec2<f32> = uvPx_1;
    let _e12: vec2<f32> = centerPx_1;
    p = (_e11 - _e12);
    let _e15: vec3<f32> = uGeoPxSize_1;
    b = (_e15.xy * 0.5f);
    let _e20: vec3<f32> = uGeoPxSize_1;
    r = _e20.z;
    let _e24: vec2<f32> = p;
    let _e26: vec2<f32> = b;
    let _e28: f32 = r;
    d = ((abs(_e24) - _e26) + vec2(_e28));
    let _e32: vec2<f32> = d;
    let _e34: vec2<f32> = d;
    let _e36: vec2<f32> = d;
    let _e38: vec2<f32> = d;
    let _e42: vec2<f32> = d;
    let _e44: vec2<f32> = d;
    let _e46: vec2<f32> = d;
    let _e48: vec2<f32> = d;
    let _e55: vec2<f32> = d;
    let _e61: vec2<f32> = d;
    let _e67: f32 = r;
    sdf = ((min(max(_e46.x, _e48.y), 0f) + length(max(_e61, vec2(0f)))) - _e67);
    let _e70: f32 = sdf;
    output = _e70;
    let _e71: f32 = output;
    return _e71;
}

fn mc_math_calculateLighting(uv: vec2<f32>, normal: vec3<f32>, uDirectionalLightDir: vec3<f32>, uDirectionalLightIntensity: f32) -> f32 {
    var uv_1: vec2<f32>;
    var normal_1: vec3<f32>;
    var uDirectionalLightDir_1: vec3<f32>;
    var uDirectionalLightIntensity_1: f32;
    var output: f32 = 0f;
    var normalizedLightDir: vec3<f32>;
    var dotProduct: f32;
    var reflectionAngle: f32;
    var angleFactor: f32;
    var adjustedIntensity: f32;
    var lighting: f32;

    uv_1 = uv;
    normal_1 = normal;
    uDirectionalLightDir_1 = uDirectionalLightDir;
    uDirectionalLightIntensity_1 = uDirectionalLightIntensity;
    let _e12: vec3<f32> = uDirectionalLightDir_1;
    normalizedLightDir = normalize(_e12);
    let _e17: vec3<f32> = normal_1;
    let _e18: vec3<f32> = normalizedLightDir;
    dotProduct = dot(_e17, _e18);
    let _e25: f32 = dotProduct;
    let _e34: f32 = dotProduct;
    reflectionAngle = acos(clamp(_e34, -1f, 1f));
    let _e42: f32 = reflectionAngle;
    angleFactor = (1f - (_e42 / 1.570795f));
    let _e49: f32 = uDirectionalLightIntensity_1;
    let _e50: f32 = angleFactor;
    adjustedIntensity = (_e49 * _e50);
    let _e55: f32 = adjustedIntensity;
    adjustedIntensity = max(_e55, 0f);
    let _e60: f32 = dotProduct;
    let _e63: f32 = adjustedIntensity;
    lighting = (max(_e60, 0f) * _e63);
    let _e66: f32 = lighting;
    output = _e66;
    let _e67: f32 = output;
    return _e67;
}

fn mc_math_calculateNormal(uv: vec2<f32>, uvPx: vec2<f32>, centerPx: vec2<f32>, uGeoPxSize: vec3<f32>, uShapeEdgePx: f32) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var uvPx_1: vec2<f32>;
    var centerPx_1: vec2<f32>;
    var uGeoPxSize_1: vec3<f32>;
    var uShapeEdgePx_1: f32;
    var output: vec3<f32> = vec3(0f);
    var p: vec2<f32>;
    var b: vec2<f32>;
    var r: f32;
    var edge: f32;
    var eps: f32 = 1f;
    var dRight: vec2<f32>;
    var dLeft: vec2<f32>;
    var dTop: vec2<f32>;
    var dBottom: vec2<f32>;
    var sdfRight: f32;
    var sdfLeft: f32;
    var sdfTop: f32;
    var sdfBottom: f32;
    var xyGradient: vec2<f32>;
    var normal: vec3<f32>;

    uv_1 = uv;
    uvPx_1 = uvPx;
    centerPx_1 = centerPx;
    uGeoPxSize_1 = uGeoPxSize;
    uShapeEdgePx_1 = uShapeEdgePx;
    let _e13: vec2<f32> = uvPx_1;
    let _e14: vec2<f32> = centerPx_1;
    p = (_e13 - _e14);
    let _e17: vec3<f32> = uGeoPxSize_1;
    b = (_e17.xy * 0.5f);
    let _e22: vec3<f32> = uGeoPxSize_1;
    r = _e22.z;
    let _e25: f32 = uShapeEdgePx_1;
    edge = _e25;
    let _e29: vec2<f32> = p;
    let _e30: f32 = eps;
    let _e34: vec2<f32> = p;
    let _e35: f32 = eps;
    let _e40: vec2<f32> = b;
    let _e42: f32 = r;
    dRight = ((abs((_e34 + vec2<f32>(_e35, 0f))) - _e40) + vec2(_e42));
    let _e46: vec2<f32> = p;
    let _e47: f32 = eps;
    let _e51: vec2<f32> = p;
    let _e52: f32 = eps;
    let _e57: vec2<f32> = b;
    let _e59: f32 = r;
    dLeft = ((abs((_e51 - vec2<f32>(_e52, 0f))) - _e57) + vec2(_e59));
    let _e63: vec2<f32> = p;
    let _e65: f32 = eps;
    let _e68: vec2<f32> = p;
    let _e70: f32 = eps;
    let _e74: vec2<f32> = b;
    let _e76: f32 = r;
    dTop = ((abs((_e68 + vec2<f32>(0f, _e70))) - _e74) + vec2(_e76));
    let _e80: vec2<f32> = p;
    let _e82: f32 = eps;
    let _e85: vec2<f32> = p;
    let _e87: f32 = eps;
    let _e91: vec2<f32> = b;
    let _e93: f32 = r;
    dBottom = ((abs((_e85 - vec2<f32>(0f, _e87))) - _e91) + vec2(_e93));
    let _e97: vec2<f32> = dRight;
    let _e99: vec2<f32> = dRight;
    let _e101: vec2<f32> = dRight;
    let _e103: vec2<f32> = dRight;
    let _e107: vec2<f32> = dRight;
    let _e109: vec2<f32> = dRight;
    let _e111: vec2<f32> = dRight;
    let _e113: vec2<f32> = dRight;
    let _e120: vec2<f32> = dRight;
    let _e126: vec2<f32> = dRight;
    let _e132: f32 = r;
    sdfRight = ((min(max(_e111.x, _e113.y), 0f) + length(max(_e126, vec2(0f)))) - _e132);
    let _e135: vec2<f32> = dLeft;
    let _e137: vec2<f32> = dLeft;
    let _e139: vec2<f32> = dLeft;
    let _e141: vec2<f32> = dLeft;
    let _e145: vec2<f32> = dLeft;
    let _e147: vec2<f32> = dLeft;
    let _e149: vec2<f32> = dLeft;
    let _e151: vec2<f32> = dLeft;
    let _e158: vec2<f32> = dLeft;
    let _e164: vec2<f32> = dLeft;
    let _e170: f32 = r;
    sdfLeft = ((min(max(_e149.x, _e151.y), 0f) + length(max(_e164, vec2(0f)))) - _e170);
    let _e173: vec2<f32> = dTop;
    let _e175: vec2<f32> = dTop;
    let _e177: vec2<f32> = dTop;
    let _e179: vec2<f32> = dTop;
    let _e183: vec2<f32> = dTop;
    let _e185: vec2<f32> = dTop;
    let _e187: vec2<f32> = dTop;
    let _e189: vec2<f32> = dTop;
    let _e196: vec2<f32> = dTop;
    let _e202: vec2<f32> = dTop;
    let _e208: f32 = r;
    sdfTop = ((min(max(_e187.x, _e189.y), 0f) + length(max(_e202, vec2(0f)))) - _e208);
    let _e211: vec2<f32> = dBottom;
    let _e213: vec2<f32> = dBottom;
    let _e215: vec2<f32> = dBottom;
    let _e217: vec2<f32> = dBottom;
    let _e221: vec2<f32> = dBottom;
    let _e223: vec2<f32> = dBottom;
    let _e225: vec2<f32> = dBottom;
    let _e227: vec2<f32> = dBottom;
    let _e234: vec2<f32> = dBottom;
    let _e240: vec2<f32> = dBottom;
    let _e246: f32 = r;
    sdfBottom = ((min(max(_e225.x, _e227.y), 0f) + length(max(_e240, vec2(0f)))) - _e246);
    let _e249: f32 = sdfRight;
    let _e250: f32 = sdfLeft;
    let _e254: f32 = sdfTop;
    let _e255: f32 = sdfBottom;
    xyGradient = vec2<f32>(((_e249 - _e250) * 0.5f), ((_e254 - _e255) * 0.5f));
    let _e261: vec2<f32> = xyGradient;
    let _e266: vec2<f32> = xyGradient;
    normal = normalize(vec3<f32>(_e266.x, _e266.y, 1f));
    let _e273: vec3<f32> = normal;
    output = _e273;
    let _e274: vec3<f32> = output;
    return _e274;
}

fn mc_math_centerPx(uv: vec2<f32>, uGeoPxSize: vec3<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var uGeoPxSize_1: vec3<f32>;
    var output: vec2<f32> = vec2(0f);
    var centerPx: vec2<f32>;

    uv_1 = uv;
    uGeoPxSize_1 = uGeoPxSize;
    let _e7: vec3<f32> = uGeoPxSize_1;
    centerPx = (_e7.xy * 0.5f);
    let _e13: vec3<f32> = uGeoPxSize_1;
    let _e15: vec2<f32> = centerPx;
    centerPx.y = (_e13.y - _e15.y);
    let _e18: vec2<f32> = centerPx;
    output = _e18;
    let _e19: vec2<f32> = output;
    return _e19;
}

fn mc_math_finalColor(uv: vec2<f32>, uGlassColor: vec4<f32>, lighting: f32, luma: f32, shapeSdf: f32, uSaturation: f32, uBrightness: f32, uAlpha: f32, uStrength: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var uGlassColor_1: vec4<f32>;
    var lighting_1: f32;
    var luma_1: f32;
    var shapeSdf_1: f32;
    var uSaturation_1: f32;
    var uBrightness_1: f32;
    var uAlpha_1: f32;
    var uStrength_1: f32;
    var output: vec4<f32> = vec4(0f);
    var hsv: vec3<f32>;
    var K: vec4<f32> = vec4<f32>(0f, -0.33333334f, 0.6666667f, -1f);
    var p: vec4<f32>;
    var q: vec4<f32>;
    var d: f32;
    var e: f32 = 0.0000000001f;
    var rgb: vec3<f32>;
    var K_1: vec4<f32> = vec4<f32>(1f, 0.6666667f, 0.33333334f, 3f);
    var p_1: vec3<f32>;
    var luminance: f32;
    var adjustedSat: vec3<f32>;
    var adjustedBright: vec3<f32>;
    var finalAlpha: f32;
    var outColor: vec4<f32>;

    uv_1 = uv;
    uGlassColor_1 = uGlassColor;
    lighting_1 = lighting;
    luma_1 = luma;
    shapeSdf_1 = shapeSdf;
    uSaturation_1 = uSaturation;
    uBrightness_1 = uBrightness;
    uAlpha_1 = uAlpha;
    uStrength_1 = uStrength;
    {
        let _e34: vec4<f32> = uGlassColor_1;
        let _e35: vec2<f32> = _e34.zy;
        let _e36: vec4<f32> = K;
        let _e37: vec2<f32> = _e36.wz;
        let _e43: vec4<f32> = uGlassColor_1;
        let _e44: vec2<f32> = _e43.yz;
        let _e45: vec4<f32> = K;
        let _e46: vec2<f32> = _e45.xy;
        let _e52: vec4<f32> = uGlassColor_1;
        let _e54: vec4<f32> = uGlassColor_1;
        let _e56: vec4<f32> = uGlassColor_1;
        let _e58: vec4<f32> = uGlassColor_1;
        let _e61: vec4<f32> = uGlassColor_1;
        let _e62: vec2<f32> = _e61.zy;
        let _e63: vec4<f32> = K;
        let _e64: vec2<f32> = _e63.wz;
        let _e70: vec4<f32> = uGlassColor_1;
        let _e71: vec2<f32> = _e70.yz;
        let _e72: vec4<f32> = K;
        let _e73: vec2<f32> = _e72.xy;
        let _e79: vec4<f32> = uGlassColor_1;
        let _e81: vec4<f32> = uGlassColor_1;
        let _e83: vec4<f32> = uGlassColor_1;
        let _e85: vec4<f32> = uGlassColor_1;
        p = mix(vec4<f32>(_e62.x, _e62.y, _e64.x, _e64.y), vec4<f32>(_e71.x, _e71.y, _e73.x, _e73.y), vec4(step(_e83.z, _e85.y)));
        let _e91: vec4<f32> = p;
        let _e92: vec3<f32> = _e91.xyw;
        let _e93: vec4<f32> = uGlassColor_1;
        let _e99: vec4<f32> = uGlassColor_1;
        let _e101: vec4<f32> = p;
        let _e102: vec3<f32> = _e101.yzx;
        let _e107: vec4<f32> = p;
        let _e109: vec4<f32> = uGlassColor_1;
        let _e111: vec4<f32> = p;
        let _e113: vec4<f32> = uGlassColor_1;
        let _e116: vec4<f32> = p;
        let _e117: vec3<f32> = _e116.xyw;
        let _e118: vec4<f32> = uGlassColor_1;
        let _e124: vec4<f32> = uGlassColor_1;
        let _e126: vec4<f32> = p;
        let _e127: vec3<f32> = _e126.yzx;
        let _e132: vec4<f32> = p;
        let _e134: vec4<f32> = uGlassColor_1;
        let _e136: vec4<f32> = p;
        let _e138: vec4<f32> = uGlassColor_1;
        q = mix(vec4<f32>(_e117.x, _e117.y, _e117.z, _e118.x), vec4<f32>(_e124.x, _e127.x, _e127.y, _e127.z), vec4(step(_e136.x, _e138.x)));
        let _e144: vec4<f32> = q;
        let _e146: vec4<f32> = q;
        let _e148: vec4<f32> = q;
        let _e150: vec4<f32> = q;
        let _e152: vec4<f32> = q;
        d = (_e144.x - min(_e150.w, _e152.y));
        let _e159: vec4<f32> = q;
        let _e161: vec4<f32> = q;
        let _e163: vec4<f32> = q;
        let _e167: f32 = d;
        let _e169: f32 = e;
        let _e173: vec4<f32> = q;
        let _e175: vec4<f32> = q;
        let _e177: vec4<f32> = q;
        let _e181: f32 = d;
        let _e183: f32 = e;
        let _e188: f32 = d;
        let _e189: vec4<f32> = q;
        let _e191: f32 = e;
        let _e194: vec4<f32> = q;
        hsv = vec3<f32>(abs((_e173.z + ((_e175.w - _e177.y) / ((6f * _e181) + _e183)))), (_e188 / (_e189.x + _e191)), _e194.x);
    }
    let _e198: vec3<f32> = hsv;
    let _e200: f32 = lighting_1;
    let _e204: f32 = luma_1;
    hsv.z = (_e198.z + ((_e200 * 1.2f) * (1f - (_e204 * 0.5f))));
    let _e211: vec3<f32> = hsv;
    let _e215: vec3<f32> = hsv;
    hsv.z = clamp(_e215.z, 0f, 1f);
    {
        let _e231: vec3<f32> = hsv;
        let _e233: vec4<f32> = K_1;
        let _e236: vec3<f32> = hsv;
        let _e238: vec4<f32> = K_1;
        let _e244: vec4<f32> = K_1;
        let _e247: vec3<f32> = hsv;
        let _e249: vec4<f32> = K_1;
        let _e252: vec3<f32> = hsv;
        let _e254: vec4<f32> = K_1;
        let _e260: vec4<f32> = K_1;
        p_1 = abs(((fract((_e252.xxx + _e254.xyz)) * 6f) - _e260.www));
        let _e265: vec3<f32> = hsv;
        let _e267: vec4<f32> = K_1;
        let _e269: vec3<f32> = p_1;
        let _e270: vec4<f32> = K_1;
        let _e275: vec3<f32> = p_1;
        let _e276: vec4<f32> = K_1;
        let _e284: vec3<f32> = hsv;
        let _e286: vec4<f32> = K_1;
        let _e288: vec3<f32> = p_1;
        let _e289: vec4<f32> = K_1;
        let _e294: vec3<f32> = p_1;
        let _e295: vec4<f32> = K_1;
        let _e303: vec3<f32> = hsv;
        rgb = (_e265.z * mix(_e286.xxx, clamp((_e294 - _e295.xxx), vec3(0f), vec3(1f)), vec3(_e303.y)));
    }
    let _e313: vec3<f32> = rgb;
    luminance = dot(_e313, vec3<f32>(0.2125f, 0.7153f, 0.0721f));
    let _e320: f32 = uSaturation_1;
    let _e321: vec3<f32> = rgb;
    let _e324: f32 = uSaturation_1;
    let _e326: f32 = luminance;
    adjustedSat = ((_e320 * _e321) + ((1f - _e324) * vec3(_e326)));
    let _e331: vec3<f32> = adjustedSat;
    let _e332: f32 = uBrightness_1;
    adjustedBright = (_e331 + vec3(_e332));
    let _e338: f32 = shapeSdf_1;
    let _e342: f32 = shapeSdf_1;
    finalAlpha = smoothstep(0f, 10f, -(_e342));
    let _e346: f32 = finalAlpha;
    let _e347: f32 = uAlpha_1;
    let _e348: f32 = uStrength_1;
    finalAlpha = (_e346 * (_e347 * _e348));
    let _e351: vec3<f32> = adjustedBright;
    let _e352: f32 = finalAlpha;
    outColor = vec4<f32>(_e351.x, _e351.y, _e351.z, _e352);
    let _e358: vec4<f32> = outColor;
    let _e360: vec4<f32> = outColor;
    let _e362: vec4<f32> = outColor;
    let _e364: vec3<f32> = (_e360.xyz * _e362.w);
    outColor.x = _e364.x;
    outColor.y = _e364.y;
    outColor.z = _e364.z;
    let _e371: vec4<f32> = outColor;
    output = _e371;
    let _e372: vec4<f32> = output;
    return _e372;
}

fn mc_math_luma(uv: vec2<f32>, uGlassColor: vec4<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var uGlassColor_1: vec4<f32>;
    var output: f32 = 0f;
    var luma: f32;

    uv_1 = uv;
    uGlassColor_1 = uGlassColor;
    let _e7: vec4<f32> = uGlassColor_1;
    let _e13: vec4<f32> = uGlassColor_1;
    luma = dot(_e13.xyz, vec3<f32>(0.2126f, 0.7152f, 0.0722f));
    let _e21: f32 = luma;
    output = _e21;
    let _e22: f32 = output;
    return _e22;
}

fn mc_math_shapeSdf(uv: vec2<f32>, baseSdf: f32, uShapeEdgePx: f32, uShapeEdgePow: f32) -> f32 {
    var uv_1: vec2<f32>;
    var baseSdf_1: f32;
    var uShapeEdgePx_1: f32;
    var uShapeEdgePow_1: f32;
    var output: f32 = 0f;
    var d: f32;
    var edge: f32;
    var per: f32;
    var t: f32;
    var t2_: f32;
    var t3_: f32;
    var t4_: f32;
    var t5_: f32;
    var t6_: f32;
    var t7_: f32;
    var circlePow: f32;

    uv_1 = uv;
    baseSdf_1 = baseSdf;
    uShapeEdgePx_1 = uShapeEdgePx;
    uShapeEdgePow_1 = uShapeEdgePow;
    let _e11: f32 = baseSdf_1;
    d = _e11;
    let _e13: f32 = uShapeEdgePx_1;
    edge = _e13;
    let _e15: f32 = d;
    let _e16: f32 = edge;
    if (_e15 < -(_e16)) {
        {
            let _e19: f32 = edge;
            d = -(_e19);
        }
    } else {
        let _e21: f32 = d;
        if (_e21 < 0f) {
            {
                let _e24: f32 = d;
                let _e26: f32 = edge;
                per = (-(_e24) / _e26);
                let _e32: f32 = per;
                let _e40: f32 = per;
                t = pow(clamp(_e40, 0f, 1f), 0.5f);
                let _e52: f32 = t;
                t = mix(0.5f, 1f, _e52);
                let _e57: f32 = t;
                t = clamp(_e57, 0f, 1f);
                let _e61: f32 = t;
                let _e62: f32 = t;
                t2_ = (_e61 * _e62);
                let _e65: f32 = t2_;
                let _e66: f32 = t;
                t3_ = (_e65 * _e66);
                let _e69: f32 = t3_;
                let _e70: f32 = t;
                t4_ = (_e69 * _e70);
                let _e73: f32 = t4_;
                let _e74: f32 = t;
                t5_ = (_e73 * _e74);
                let _e77: f32 = t5_;
                let _e78: f32 = t;
                t6_ = (_e77 * _e78);
                let _e81: f32 = t6_;
                let _e82: f32 = t;
                t7_ = (_e81 * _e82);
                let _e87: f32 = t7_;
                let _e90: f32 = t6_;
                let _e94: f32 = t5_;
                let _e98: f32 = t4_;
                t = ((((-20f * _e87) + (70f * _e90)) - (84f * _e94)) + (35f * _e98));
                let _e101: f32 = t;
                t = ((_e101 - 0.5f) * 2f);
                let _e108: f32 = t;
                let _e112: f32 = t;
                let _e114: f32 = uShapeEdgePow_1;
                circlePow = (1f - pow((1f - _e112), _e114));
                let _e118: f32 = circlePow;
                let _e120: f32 = edge;
                d = (-(_e118) * _e120);
            }
        }
    }
    let _e122: f32 = d;
    output = _e122;
    let _e123: f32 = output;
    return _e123;
}

fn mc_math_uvToPx(uv: vec2<f32>, uGeoPxSize: vec3<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var uGeoPxSize_1: vec3<f32>;
    var output: vec2<f32> = vec2(0f);
    var uvPx: vec2<f32>;

    uv_1 = uv;
    uGeoPxSize_1 = uGeoPxSize;
    let _e7: vec2<f32> = uv_1;
    let _e8: vec3<f32> = uGeoPxSize_1;
    uvPx = (_e7 * _e8.xy);
    let _e12: vec2<f32> = uvPx;
    output = _e12;
    let _e13: vec2<f32> = output;
    return _e13;
}


@vertex
fn vs_main(@location(0) position: vec3f) -> VSOut {
    var out: VSOut;

    // Local UV in [0,1] based on geometry size.
    out.uv = (position.xy / params.geo_size) + vec2f(0.5, 0.5);

    // Geometry vertices are in local pixel units centered at (0,0). Apply center translation in pixels.
    let p = position.xy + params.center;

    // Convert pixels to clip space (assumes target_size is in pixels and (0,0) is the target center).
    let half = params.target_size * 0.5;
    let ndc = vec2f(p.x / half.x, p.y / half.y);
    out.position = vec4f(ndc, position.z, 1.0);
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_math_uvToPx_out: vec2f;
    {
        let uGeoPxSize = vec3f(200, 200, 20);
        var output: vec2f;
        output = mc_math_uvToPx(in.uv, uGeoPxSize);
        mc_math_uvToPx_out = output;
    }
    var mc_math_centerPx_out: vec2f;
    {
        let uGeoPxSize = vec3f(200, 200, 20);
        var output: vec2f;
        output = mc_math_centerPx(in.uv, uGeoPxSize);
        mc_math_centerPx_out = output;
    }
    var mc_math_calculateNormal_out: vec3f;
    {
        let uvPx = mc_math_uvToPx_out;
        let centerPx = mc_math_centerPx_out;
        let uGeoPxSize = vec3f(200, 200, 20);
        let uShapeEdgePx = 30.0;
        var output: vec3f;
        output = mc_math_calculateNormal(in.uv, uvPx, centerPx, uGeoPxSize, uShapeEdgePx);
        mc_math_calculateNormal_out = output;
    }
    var mc_math_calculateLighting_out: f32;
    {
        let normal = mc_math_calculateNormal_out;
        let uDirectionalLightDir = vec3f(0.5, -0.7, 0.5);
        let uDirectionalLightIntensity = 1.0;
        var output: f32;
        output = mc_math_calculateLighting(in.uv, normal, uDirectionalLightDir, uDirectionalLightIntensity);
        mc_math_calculateLighting_out = output;
    }
    var mc_math_luma_out: f32;
    {
        let uGlassColor = vec4f(0.27, 0.285, 0.3, 0.3);
        var output: f32;
        output = mc_math_luma(in.uv, uGlassColor);
        mc_math_luma_out = output;
    }
    var mc_math_boxSdf_out: f32;
    {
        let uvPx = mc_math_uvToPx_out;
        let centerPx = mc_math_centerPx_out;
        let uGeoPxSize = vec3f(200, 200, 20);
        var output: f32;
        output = mc_math_boxSdf(in.uv, uvPx, centerPx, uGeoPxSize);
        mc_math_boxSdf_out = output;
    }
    var mc_math_shapeSdf_out: f32;
    {
        let baseSdf = mc_math_boxSdf_out;
        let uShapeEdgePx = 30.0;
        let uShapeEdgePow = 2.0;
        var output: f32;
        output = mc_math_shapeSdf(in.uv, baseSdf, uShapeEdgePx, uShapeEdgePow);
        mc_math_shapeSdf_out = output;
    }
    var mc_math_finalColor_out: vec4f;
    {
        let uGlassColor = vec4f(0.27, 0.285, 0.3, 0.3);
        let lighting = mc_math_calculateLighting_out;
        let luma = mc_math_luma_out;
        let shapeSdf = mc_math_shapeSdf_out;
        let uSaturation = 1.0;
        let uBrightness = 0.0;
        let uAlpha = 1.0;
        let uStrength = 1.0;
        var output: vec4f;
        output = mc_math_finalColor(in.uv, uGlassColor, lighting, luma, shapeSdf, uSaturation, uBrightness, uAlpha, uStrength);
        mc_math_finalColor_out = output;
    }
    return mc_math_finalColor_out;
}
