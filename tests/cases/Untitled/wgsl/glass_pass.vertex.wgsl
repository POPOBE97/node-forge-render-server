
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
};

// --- Extra WGSL declarations (generated) ---
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

fn mc_math_calculateNormal(uv: vec2<f32>, uvPx: vec2<f32>, centerPx: vec2<f32>, uShapeEdgePx: f32, uGeoPxSize: vec2<f32>, r: f32) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var uvPx_1: vec2<f32>;
    var centerPx_1: vec2<f32>;
    var uShapeEdgePx_1: f32;
    var uGeoPxSize_1: vec2<f32>;
    var r_1: f32;
    var output: vec3<f32> = vec3(0f);
    var p: vec2<f32>;
    var b: vec2<f32>;
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
    uShapeEdgePx_1 = uShapeEdgePx;
    uGeoPxSize_1 = uGeoPxSize;
    r_1 = r;
    let _e15: vec2<f32> = uvPx_1;
    let _e16: vec2<f32> = centerPx_1;
    p = (_e15 - _e16);
    let _e19: vec2<f32> = uGeoPxSize_1;
    b = (_e19.xy * 0.5f);
    let _e24: f32 = uShapeEdgePx_1;
    edge = _e24;
    let _e28: vec2<f32> = p;
    let _e29: f32 = eps;
    let _e33: vec2<f32> = p;
    let _e34: f32 = eps;
    let _e39: vec2<f32> = b;
    let _e41: f32 = r_1;
    dRight = ((abs((_e33 + vec2<f32>(_e34, 0f))) - _e39) + vec2(_e41));
    let _e45: vec2<f32> = p;
    let _e46: f32 = eps;
    let _e50: vec2<f32> = p;
    let _e51: f32 = eps;
    let _e56: vec2<f32> = b;
    let _e58: f32 = r_1;
    dLeft = ((abs((_e50 - vec2<f32>(_e51, 0f))) - _e56) + vec2(_e58));
    let _e62: vec2<f32> = p;
    let _e64: f32 = eps;
    let _e67: vec2<f32> = p;
    let _e69: f32 = eps;
    let _e73: vec2<f32> = b;
    let _e75: f32 = r_1;
    dTop = ((abs((_e67 + vec2<f32>(0f, _e69))) - _e73) + vec2(_e75));
    let _e79: vec2<f32> = p;
    let _e81: f32 = eps;
    let _e84: vec2<f32> = p;
    let _e86: f32 = eps;
    let _e90: vec2<f32> = b;
    let _e92: f32 = r_1;
    dBottom = ((abs((_e84 - vec2<f32>(0f, _e86))) - _e90) + vec2(_e92));
    let _e96: vec2<f32> = dRight;
    let _e98: vec2<f32> = dRight;
    let _e100: vec2<f32> = dRight;
    let _e102: vec2<f32> = dRight;
    let _e106: vec2<f32> = dRight;
    let _e108: vec2<f32> = dRight;
    let _e110: vec2<f32> = dRight;
    let _e112: vec2<f32> = dRight;
    let _e119: vec2<f32> = dRight;
    let _e125: vec2<f32> = dRight;
    let _e131: f32 = r_1;
    sdfRight = ((min(max(_e110.x, _e112.y), 0f) + length(max(_e125, vec2(0f)))) - _e131);
    let _e134: vec2<f32> = dLeft;
    let _e136: vec2<f32> = dLeft;
    let _e138: vec2<f32> = dLeft;
    let _e140: vec2<f32> = dLeft;
    let _e144: vec2<f32> = dLeft;
    let _e146: vec2<f32> = dLeft;
    let _e148: vec2<f32> = dLeft;
    let _e150: vec2<f32> = dLeft;
    let _e157: vec2<f32> = dLeft;
    let _e163: vec2<f32> = dLeft;
    let _e169: f32 = r_1;
    sdfLeft = ((min(max(_e148.x, _e150.y), 0f) + length(max(_e163, vec2(0f)))) - _e169);
    let _e172: vec2<f32> = dTop;
    let _e174: vec2<f32> = dTop;
    let _e176: vec2<f32> = dTop;
    let _e178: vec2<f32> = dTop;
    let _e182: vec2<f32> = dTop;
    let _e184: vec2<f32> = dTop;
    let _e186: vec2<f32> = dTop;
    let _e188: vec2<f32> = dTop;
    let _e195: vec2<f32> = dTop;
    let _e201: vec2<f32> = dTop;
    let _e207: f32 = r_1;
    sdfTop = ((min(max(_e186.x, _e188.y), 0f) + length(max(_e201, vec2(0f)))) - _e207);
    let _e210: vec2<f32> = dBottom;
    let _e212: vec2<f32> = dBottom;
    let _e214: vec2<f32> = dBottom;
    let _e216: vec2<f32> = dBottom;
    let _e220: vec2<f32> = dBottom;
    let _e222: vec2<f32> = dBottom;
    let _e224: vec2<f32> = dBottom;
    let _e226: vec2<f32> = dBottom;
    let _e233: vec2<f32> = dBottom;
    let _e239: vec2<f32> = dBottom;
    let _e245: f32 = r_1;
    sdfBottom = ((min(max(_e224.x, _e226.y), 0f) + length(max(_e239, vec2(0f)))) - _e245);
    let _e248: f32 = sdfRight;
    let _e249: f32 = sdfLeft;
    let _e253: f32 = sdfTop;
    let _e254: f32 = sdfBottom;
    xyGradient = vec2<f32>(((_e248 - _e249) * 0.5f), ((_e253 - _e254) * 0.5f));
    let _e260: vec2<f32> = xyGradient;
    let _e265: vec2<f32> = xyGradient;
    normal = normalize(vec3<f32>(_e265.x, _e265.y, 1f));
    let _e272: vec3<f32> = normal;
    output = _e272;
    let _e273: vec3<f32> = output;
    return _e273;
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

 let p_local = position;

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