
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

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;

// --- Extra WGSL declarations (generated) ---
fn mc_MathClosure_13_(uv: vec2<f32>, size: f32) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var size_1: f32;
    var output: vec2<f32> = vec2(0f);

    uv_1 = uv;
    size_1 = size;
    let _e7: f32 = size_1;
    output = vec2((_e7 * 2f));
    let _e11: vec2<f32> = output;
    return _e11;
}

fn mc_MathClosure_8_(uv: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    output = vec4(20f);
    let _e7: vec4<f32> = output;
    return _e7;
}

fn mc_MathClosure_9_(uv: vec2<f32>, sdfCircle: f32, sdfBox: f32, t: f32) -> f32 {
    var uv_1: vec2<f32>;
    var sdfCircle_1: f32;
    var sdfBox_1: f32;
    var t_1: f32;
    var output: f32 = 0f;

    uv_1 = uv;
    sdfCircle_1 = sdfCircle;
    sdfBox_1 = sdfBox;
    t_1 = t;
    let _e14: f32 = sdfCircle_1;
    let _e15: f32 = sdfBox_1;
    let _e16: f32 = t_1;
    output = mix(_e14, _e15, _e16);
    let _e18: f32 = output;
    return _e18;
}

fn mc_math_artisticMapping(uv: vec2<f32>, combinedLight: f32, uStrength: f32, sdf: f32, size: f32) -> f32 {
    var uv_1: vec2<f32>;
    var combinedLight_1: f32;
    var uStrength_1: f32;
    var sdf_1: f32;
    var size_1: f32;
    var output: f32 = 0f;
    var finalAlpha: f32;
    var nf: f32;
    var limit: f32;

    uv_1 = uv;
    combinedLight_1 = combinedLight;
    uStrength_1 = uStrength;
    sdf_1 = sdf;
    size_1 = size;
    let _e13: f32 = combinedLight_1;
    let _e14: f32 = combinedLight_1;
    finalAlpha = (_e13 * _e14);
    let _e20: f32 = finalAlpha;
    finalAlpha = clamp(_e20, 0f, 1f);
    let _e26: f32 = size_1;
    nf = max(_e26, 0.00001f);
    let _e31: f32 = sdf_1;
    let _e32: f32 = nf;
    limit = (1f + (_e31 / _e32));
    let _e36: f32 = finalAlpha;
    let _e40: f32 = limit;
    let _e48: f32 = limit;
    finalAlpha = (_e36 * max((1f - pow(_e48, 8f)), 0f));
    let _e55: f32 = finalAlpha;
    let _e56: f32 = uStrength_1;
    finalAlpha = (_e55 * _e56);
    let _e58: f32 = finalAlpha;
    output = _e58;
    let _e59: f32 = output;
    return _e59;
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

fn mc_math_ring1Light(uv: vec2<f32>, uRing1_: vec3<f32>, sdf: f32, size: f32) -> f32 {
    var uv_1: vec2<f32>;
    var uRing1_1: vec3<f32>;
    var sdf_1: f32;
    var size_1: f32;
    var output: f32 = 0f;
    var nf: f32;
    var lit: f32;

    uv_1 = uv;
    uRing1_1 = uRing1_;
    sdf_1 = sdf;
    size_1 = size;
    let _e13: f32 = size_1;
    nf = max(_e13, 0.00001f);
    let _e18: f32 = sdf_1;
    let _e19: f32 = nf;
    lit = (1f + (_e18 / _e19));
    let _e26: f32 = lit;
    let _e27: vec3<f32> = uRing1_1;
    let _e30: vec3<f32> = uRing1_1;
    let _e33: f32 = lit;
    let _e34: vec3<f32> = uRing1_1;
    let _e37: vec3<f32> = uRing1_1;
    let _e43: f32 = lit;
    let _e44: vec3<f32> = uRing1_1;
    let _e47: vec3<f32> = uRing1_1;
    let _e50: f32 = lit;
    let _e51: vec3<f32> = uRing1_1;
    let _e54: vec3<f32> = uRing1_1;
    lit = (1f - smoothstep(0f, 1f, abs(((_e50 - _e51.x) * _e54.y))));
    let _e60: f32 = lit;
    let _e61: vec3<f32> = uRing1_1;
    output = (_e60 * _e61.z);
    let _e64: f32 = output;
    return _e64;
}

fn mc_math_ring2Light(uv: vec2<f32>, uRing2_: vec3<f32>, sdf: f32, size: f32) -> f32 {
    var uv_1: vec2<f32>;
    var uRing2_1: vec3<f32>;
    var sdf_1: f32;
    var size_1: f32;
    var output: f32 = 0f;
    var nf: f32;
    var lit: f32;

    uv_1 = uv;
    uRing2_1 = uRing2_;
    sdf_1 = sdf;
    size_1 = size;
    let _e13: f32 = size_1;
    nf = max(_e13, 0.00001f);
    let _e18: f32 = sdf_1;
    let _e19: f32 = nf;
    lit = (1f + (_e18 / _e19));
    let _e26: f32 = lit;
    let _e27: vec3<f32> = uRing2_1;
    let _e30: vec3<f32> = uRing2_1;
    let _e33: f32 = lit;
    let _e34: vec3<f32> = uRing2_1;
    let _e37: vec3<f32> = uRing2_1;
    let _e43: f32 = lit;
    let _e44: vec3<f32> = uRing2_1;
    let _e47: vec3<f32> = uRing2_1;
    let _e50: f32 = lit;
    let _e51: vec3<f32> = uRing2_1;
    let _e54: vec3<f32> = uRing2_1;
    lit = (1f - smoothstep(0f, 1f, abs(((_e50 - _e51.x) * _e54.y))));
    let _e60: f32 = lit;
    let _e61: vec3<f32> = uRing2_1;
    output = (_e60 * _e61.z);
    let _e64: f32 = output;
    return _e64;
}

fn mc_math_src1Light(uv: vec2<f32>, uSrc1_: vec2<f32>, sdf: f32, size: f32) -> f32 {
    var uv_1: vec2<f32>;
    var uSrc1_1: vec2<f32>;
    var sdf_1: f32;
    var size_1: f32;
    var output: f32 = 0f;
    var nf: f32;
    var lit: f32;

    uv_1 = uv;
    uSrc1_1 = uSrc1_;
    sdf_1 = sdf;
    size_1 = size;
    let _e13: f32 = size_1;
    nf = max(_e13, 0.00001f);
    let _e18: f32 = sdf_1;
    let _e19: f32 = nf;
    lit = (1f + (_e18 / _e19));
    let _e23: f32 = lit;
    let _e25: f32 = lit;
    let _e27: vec2<f32> = uSrc1_1;
    let _e30: f32 = lit;
    let _e32: f32 = lit;
    let _e34: vec2<f32> = uSrc1_1;
    lit = exp(((-(_e30) * _e32) * _e34.x));
    let _e38: f32 = lit;
    let _e39: vec2<f32> = uSrc1_1;
    output = (_e38 * _e39.y);
    let _e42: f32 = output;
    return _e42;
}

fn mc_math_src2Light(uv: vec2<f32>, uSrc2_: vec2<f32>, sdf: f32, size: f32) -> f32 {
    var uv_1: vec2<f32>;
    var uSrc2_1: vec2<f32>;
    var sdf_1: f32;
    var size_1: f32;
    var output: f32 = 0f;
    var nf: f32;
    var lit: f32;

    uv_1 = uv;
    uSrc2_1 = uSrc2_;
    sdf_1 = sdf;
    size_1 = size;
    let _e13: f32 = size_1;
    nf = max(_e13, 0.00001f);
    let _e18: f32 = sdf_1;
    let _e19: f32 = nf;
    lit = (1f + (_e18 / _e19));
    let _e25: f32 = lit;
    let _e26: vec2<f32> = uSrc2_1;
    let _e32: f32 = lit;
    let _e33: vec2<f32> = uSrc2_1;
    let _e41: f32 = lit;
    let _e42: vec2<f32> = uSrc2_1;
    let _e48: f32 = lit;
    let _e49: vec2<f32> = uSrc2_1;
    lit = (1f - max(pow((1f - (_e48 * _e49.x)), 3f), 0f));
    let _e58: f32 = lit;
    let _e59: vec2<f32> = uSrc2_1;
    output = (_e58 * _e59.y);
    let _e62: f32 = output;
    return _e62;
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


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_13_out: vec2f;
    {
        let size = 100.0;
        var output: vec2f;
        output = mc_MathClosure_13_(in.uv, size);
        mc_MathClosure_13_out = output;
    }
    var mc_MathClosure_8_out: vec4f;
    {
        var output: vec4f;
        output = mc_MathClosure_8_(in.uv);
        mc_MathClosure_8_out = output;
    }
    var mc_MathClosure_9_out: f32;
    {
        let sdfCircle = (length(((in.uv * params.geo_size) - vec2f(100, 100))) - 100.0);
        let sdfBox = sdf2d_round_rect(((in.uv * params.geo_size) - vec2f(100, 100)), (mc_MathClosure_13_out * 0.5), mc_MathClosure_8_out);
        let t = 1.0;
        var output: f32;
        output = mc_MathClosure_9_(in.uv, sdfCircle, sdfBox, t);
        mc_MathClosure_9_out = output;
    }
    var mc_math_src1Light_out: f32;
    {
        let uSrc1 = vec2f(2.33, 0.5);
        let sdf = mc_MathClosure_9_out;
        let size = 100.0;
        var output: f32;
        output = mc_math_src1Light(in.uv, uSrc1, sdf, size);
        mc_math_src1Light_out = output;
    }
    var mc_math_src2Light_out: f32;
    {
        let uSrc2 = vec2f(1.5, 0.3);
        let sdf = mc_MathClosure_9_out;
        let size = 100.0;
        var output: f32;
        output = mc_math_src2Light(in.uv, uSrc2, sdf, size);
        mc_math_src2Light_out = output;
    }
    var mc_math_ring1Light_out: f32;
    {
        let uRing1 = vec3f(0.8, 5, 0.4);
        let sdf = mc_MathClosure_9_out;
        let size = 100.0;
        var output: f32;
        output = mc_math_ring1Light(in.uv, uRing1, sdf, size);
        mc_math_ring1Light_out = output;
    }
    var mc_math_ring2Light_out: f32;
    {
        let uRing2 = vec3f(0.6, 7.19, 0.3);
        let sdf = mc_MathClosure_9_out;
        let size = 100.0;
        var output: f32;
        output = mc_math_ring2Light(in.uv, uRing2, sdf, size);
        mc_math_ring2Light_out = output;
    }
    var mc_math_combineLights_out: f32;
    {
        let src1Light = mc_math_src1Light_out;
        let src2Light = mc_math_src2Light_out;
        let ring1Light = mc_math_ring1Light_out;
        let ring2Light = mc_math_ring2Light_out;
        var output: f32;
        output = mc_math_combineLights(in.uv, src1Light, src2Light, ring1Light, ring2Light);
        mc_math_combineLights_out = output;
    }
    var mc_math_artisticMapping_out: f32;
    {
        let combinedLight = mc_math_combineLights_out;
        let uStrength = 1.0;
        let sdf = mc_MathClosure_9_out;
        let size = 100.0;
        var output: f32;
        output = mc_math_artisticMapping(in.uv, combinedLight, uStrength, sdf, size);
        mc_math_artisticMapping_out = output;
    }
    var mc_math_finalColor_out: vec4f;
    {
        let uColor = vec4f(0.279, 0.2511, 0.19530001, 0.279);
        let finalAlpha = mc_math_artisticMapping_out;
        var output: vec4f;
        output = mc_math_finalColor(in.uv, uColor, finalAlpha);
        mc_math_finalColor_out = output;
    }
    return mc_math_finalColor_out;
}
