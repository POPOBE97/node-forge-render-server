
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
var pass_tex_RenderPass_mip0: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_RenderPass_mip0: sampler;

@group(1) @binding(2)
var pass_tex_Downsample_mip1: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_Downsample_mip1: sampler;

@group(1) @binding(4)
var pass_tex_Downsample_mip2: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp_Downsample_mip2: sampler;

@group(1) @binding(6)
var pass_tex_Downsample_mip3: texture_2d<f32>;

@group(1) @binding(7)
var pass_samp_Downsample_mip3: sampler;

@group(1) @binding(8)
var pass_tex_Downsample_mip4: texture_2d<f32>;

@group(1) @binding(9)
var pass_samp_Downsample_mip4: sampler;

@group(1) @binding(10)
var pass_tex_Downsample_mip5: texture_2d<f32>;

@group(1) @binding(11)
var pass_samp_Downsample_mip5: sampler;

@group(1) @binding(12)
var pass_tex_Downsample_mip6: texture_2d<f32>;

@group(1) @binding(13)
var pass_samp_Downsample_mip6: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_MathClosure_calculateMaskLinear(uv: vec2<f32>, xy: vec2<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var output: f32 = 0f;
    var textureHeight: f32 = 2400f;
    var pStart: vec3<f32> = vec3<f32>(0f, 0f, 50f);
    var pEnd: vec3<f32>;
    var qBase: vec2<f32>;
    var md: f32;
    var q: vec2<f32>;
    var p: f32;
    var m: f32;

    uv_1 = uv;
    xy_1 = xy;
    let _e15: f32 = textureHeight;
    pEnd = vec3<f32>(0f, _e15, 0f);
    let _e19: vec3<f32> = pEnd;
    let _e21: vec3<f32> = pStart;
    qBase = (_e19.xy - _e21.xy);
    let _e27: vec2<f32> = qBase;
    let _e28: vec2<f32> = qBase;
    md = dot(_e27, _e28);
    let _e31: vec2<f32> = xy_1;
    let _e32: vec3<f32> = pStart;
    q = (_e31 - _e32.xy);
    let _e38: vec2<f32> = q;
    let _e39: vec2<f32> = qBase;
    p = dot(_e38, _e39);
    let _e45: f32 = md;
    let _e47: f32 = p;
    m = smoothstep(_e45, 0f, _e47);
    let _e50: vec3<f32> = pEnd;
    let _e52: vec3<f32> = pStart;
    let _e54: vec3<f32> = pEnd;
    let _e57: f32 = m;
    m = (_e50.z + ((_e52.z - _e54.z) * _e57));
    let _e60: f32 = m;
    let _e63: f32 = m;
    m = log2((_e63 * 1.333333f));
    let _e70: f32 = m;
    output = clamp(_e70, 0f, 6f);
    let _e74: f32 = output;
    return _e74;
}

fn mc_MathClosure_fragment(uv: vec2<f32>, xy: vec2<f32>, m: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var m_1: f32;
    var output: vec4<f32> = vec4(0f);
    var coord: vec2<f32>;
    var uEnlargedTextureSize: vec2<f32> = vec2<f32>(1080f, 2400f);
    var clo: vec4<f32>;
    var cHi: vec4<f32>;
    var scale: f32;
    var mLo: f32;
    var res0_: vec2<f32>;
    var p0_: vec2<f32>;
    var i0_: vec2<i32>;
    var f0_: vec2<f32>;
    var uv00_: vec2<f32>;
    var uv10_: vec2<f32>;
    var uv01_: vec2<f32>;
    var uv11_: vec2<f32>;
    var c00_: vec4<f32>;
    var c10_: vec4<f32>;
    var c01_: vec4<f32>;
    var c11_: vec4<f32>;
    var d: vec2<f32>;
    var c: vec2<f32>;
    var x: vec2<f32>;
    var X: vec2<f32>;
    var x3_: vec2<f32>;
    var coeff: vec2<f32>;
    var w1_: vec2<f32>;
    var w2_: vec2<f32>;
    var o1_: vec2<f32>;
    var o2_: vec2<f32>;
    var resLo: vec2<f32>;
    var p_o1o1_: vec2<f32>;
    var p_o2o1_: vec2<f32>;
    var p_o1o2_: vec2<f32>;
    var p_o2o2_: vec2<f32>;
    var s00_: vec4<f32>;
    var s10_: vec4<f32>;
    var s01_: vec4<f32>;
    var s11_: vec4<f32>;
    var mHi: f32;
    var dHi: vec2<f32>;
    var cHi_1: vec2<f32>;
    var xHi: vec2<f32>;
    var XHi: vec2<f32>;
    var x3Hi: vec2<f32>;
    var coeffHi: vec2<f32>;
    var w1Hi: vec2<f32>;
    var w2Hi: vec2<f32>;
    var o1Hi: vec2<f32>;
    var o2Hi: vec2<f32>;
    var resHi: vec2<f32>;
    var h00_: vec4<f32>;
    var h10_: vec4<f32>;
    var h01_: vec4<f32>;
    var h11_: vec4<f32>;

    uv_1 = uv;
    xy_1 = xy;
    m_1 = m;
    let _e9: vec2<f32> = xy_1;
    coord = _e9;
    let _e19: f32 = m_1;
    mLo = floor(_e19);
    let _e22: f32 = mLo;
    if (_e22 < 0.1f) {
        {
            let _e25: vec2<f32> = uEnlargedTextureSize;
            res0_ = _e25;
            let _e27: vec2<f32> = coord;
            p0_ = (_e27 - vec2(0.5f));
            let _e33: vec2<f32> = p0_;
            i0_ = vec2<i32>(floor(_e33));
            let _e38: vec2<f32> = p0_;
            f0_ = fract(_e38);
            let _e41: vec2<i32> = i0_;
            let _e43: vec2<f32> = res0_;
            let _e49: vec2<i32> = i0_;
            let _e51: vec2<f32> = res0_;
            uv00_ = clamp((vec2<f32>(_e49) / _e51), vec2(0f), vec2(1f));
            let _e59: vec2<i32> = i0_;
            let _e65: vec2<f32> = res0_;
            let _e71: vec2<i32> = i0_;
            let _e77: vec2<f32> = res0_;
            uv10_ = clamp((vec2<f32>((_e71 + vec2<i32>(1i, 0i))) / _e77), vec2(0f), vec2(1f));
            let _e85: vec2<i32> = i0_;
            let _e91: vec2<f32> = res0_;
            let _e97: vec2<i32> = i0_;
            let _e103: vec2<f32> = res0_;
            uv01_ = clamp((vec2<f32>((_e97 + vec2<i32>(0i, 1i))) / _e103), vec2(0f), vec2(1f));
            let _e111: vec2<i32> = i0_;
            let _e117: vec2<f32> = res0_;
            let _e123: vec2<i32> = i0_;
            let _e129: vec2<f32> = res0_;
            uv11_ = clamp((vec2<f32>((_e123 + vec2<i32>(1i, 1i))) / _e129), vec2(0f), vec2(1f));
            let _e138: vec2<f32> = uv00_;
            let _e139: vec4<f32> = sample_pass_RenderPass_mip0_(_e138);
            c00_ = _e139;
            let _e142: vec2<f32> = uv10_;
            let _e143: vec4<f32> = sample_pass_RenderPass_mip0_(_e142);
            c10_ = _e143;
            let _e146: vec2<f32> = uv01_;
            let _e147: vec4<f32> = sample_pass_RenderPass_mip0_(_e146);
            c01_ = _e147;
            let _e150: vec2<f32> = uv11_;
            let _e151: vec4<f32> = sample_pass_RenderPass_mip0_(_e150);
            c11_ = _e151;
            let _e155: vec2<f32> = f0_;
            let _e157: vec4<f32> = c00_;
            let _e158: vec4<f32> = c10_;
            let _e159: vec2<f32> = f0_;
            let _e165: vec2<f32> = f0_;
            let _e167: vec4<f32> = c01_;
            let _e168: vec4<f32> = c11_;
            let _e169: vec2<f32> = f0_;
            let _e173: vec2<f32> = f0_;
            let _e177: vec2<f32> = f0_;
            let _e179: vec4<f32> = c00_;
            let _e180: vec4<f32> = c10_;
            let _e181: vec2<f32> = f0_;
            let _e187: vec2<f32> = f0_;
            let _e189: vec4<f32> = c01_;
            let _e190: vec4<f32> = c11_;
            let _e191: vec2<f32> = f0_;
            let _e195: vec2<f32> = f0_;
            clo = mix(mix(_e179, _e180, vec4(_e181.x)), mix(_e189, _e190, vec4(_e191.x)), vec4(_e195.y));
        }
    } else {
        {
            let _e203: f32 = mLo;
            scale = (1f / pow(2f, _e203));
            let _e206: vec2<f32> = coord;
            let _e207: f32 = scale;
            d = ((_e206 * _e207) - vec2(0.5f));
            let _e214: vec2<f32> = d;
            c = floor(_e214);
            let _e217: vec2<f32> = c;
            let _e218: vec2<f32> = d;
            x = ((_e217 - _e218) + vec2(1f));
            let _e224: vec2<f32> = d;
            let _e225: vec2<f32> = c;
            X = (_e224 - _e225);
            let _e228: vec2<f32> = x;
            let _e229: vec2<f32> = x;
            let _e231: vec2<f32> = x;
            x3_ = ((_e228 * _e229) * _e231);
            let _e235: vec2<f32> = x;
            let _e237: vec2<f32> = x;
            let _e240: vec2<f32> = x;
            coeff = ((((0.5f * _e235) * _e237) + (0.5f * _e240)) + vec2(0.166667f));
            let _e249: vec2<f32> = x3_;
            let _e251: vec2<f32> = coeff;
            w1_ = ((-0.333333f * _e249) + _e251);
            let _e255: vec2<f32> = w1_;
            w2_ = (vec2(1f) - _e255);
            let _e261: vec2<f32> = x3_;
            let _e263: vec2<f32> = coeff;
            let _e265: vec2<f32> = w1_;
            let _e267: vec2<f32> = c;
            o1_ = (((((-0.5f * _e261) + _e263) / _e265) + _e267) - vec2(0.5f));
            let _e273: vec2<f32> = X;
            let _e274: vec2<f32> = X;
            let _e276: vec2<f32> = X;
            let _e281: vec2<f32> = w2_;
            let _e283: vec2<f32> = c;
            o2_ = ((((((_e273 * _e274) * _e276) / vec2(6f)) / _e281) + _e283) + vec2(1.5f));
            let _e289: vec2<f32> = uEnlargedTextureSize;
            let _e293: f32 = mLo;
            resLo = (_e289 / vec2(pow(2f, _e293)));
            let _e298: vec2<f32> = o1_;
            let _e300: vec2<f32> = o1_;
            p_o1o1_ = (vec2<f32>(_e298.x, _e300.y) - vec2(0.5f));
            let _e307: vec2<f32> = o2_;
            let _e309: vec2<f32> = o1_;
            p_o2o1_ = (vec2<f32>(_e307.x, _e309.y) - vec2(0.5f));
            let _e316: vec2<f32> = o1_;
            let _e318: vec2<f32> = o2_;
            p_o1o2_ = (vec2<f32>(_e316.x, _e318.y) - vec2(0.5f));
            let _e325: vec2<f32> = o2_;
            let _e327: vec2<f32> = o2_;
            p_o2o2_ = (vec2<f32>(_e325.x, _e327.y) - vec2(0.5f));
            let _e338: f32 = mLo;
            if (_e338 < 1.5f) {
                {
                    let _e341: vec2<f32> = o1_;
                    let _e343: vec2<f32> = o1_;
                    let _e346: vec2<f32> = resLo;
                    let _e352: vec2<f32> = o1_;
                    let _e354: vec2<f32> = o1_;
                    let _e357: vec2<f32> = resLo;
                    let _e364: vec2<f32> = o1_;
                    let _e366: vec2<f32> = o1_;
                    let _e369: vec2<f32> = resLo;
                    let _e375: vec2<f32> = o1_;
                    let _e377: vec2<f32> = o1_;
                    let _e380: vec2<f32> = resLo;
                    let _e387: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e375.x, _e377.y) / _e380), vec2(0f), vec2(1f)));
                    s00_ = _e387;
                    let _e388: vec2<f32> = o2_;
                    let _e390: vec2<f32> = o1_;
                    let _e393: vec2<f32> = resLo;
                    let _e399: vec2<f32> = o2_;
                    let _e401: vec2<f32> = o1_;
                    let _e404: vec2<f32> = resLo;
                    let _e411: vec2<f32> = o2_;
                    let _e413: vec2<f32> = o1_;
                    let _e416: vec2<f32> = resLo;
                    let _e422: vec2<f32> = o2_;
                    let _e424: vec2<f32> = o1_;
                    let _e427: vec2<f32> = resLo;
                    let _e434: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e422.x, _e424.y) / _e427), vec2(0f), vec2(1f)));
                    s10_ = _e434;
                    let _e435: vec2<f32> = o1_;
                    let _e437: vec2<f32> = o2_;
                    let _e440: vec2<f32> = resLo;
                    let _e446: vec2<f32> = o1_;
                    let _e448: vec2<f32> = o2_;
                    let _e451: vec2<f32> = resLo;
                    let _e458: vec2<f32> = o1_;
                    let _e460: vec2<f32> = o2_;
                    let _e463: vec2<f32> = resLo;
                    let _e469: vec2<f32> = o1_;
                    let _e471: vec2<f32> = o2_;
                    let _e474: vec2<f32> = resLo;
                    let _e481: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e469.x, _e471.y) / _e474), vec2(0f), vec2(1f)));
                    s01_ = _e481;
                    let _e482: vec2<f32> = o2_;
                    let _e484: vec2<f32> = o2_;
                    let _e487: vec2<f32> = resLo;
                    let _e493: vec2<f32> = o2_;
                    let _e495: vec2<f32> = o2_;
                    let _e498: vec2<f32> = resLo;
                    let _e505: vec2<f32> = o2_;
                    let _e507: vec2<f32> = o2_;
                    let _e510: vec2<f32> = resLo;
                    let _e516: vec2<f32> = o2_;
                    let _e518: vec2<f32> = o2_;
                    let _e521: vec2<f32> = resLo;
                    let _e528: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e516.x, _e518.y) / _e521), vec2(0f), vec2(1f)));
                    s11_ = _e528;
                }
            } else {
                let _e529: f32 = mLo;
                if (_e529 < 2.5f) {
                    {
                        let _e532: vec2<f32> = o1_;
                        let _e534: vec2<f32> = o1_;
                        let _e537: vec2<f32> = resLo;
                        let _e543: vec2<f32> = o1_;
                        let _e545: vec2<f32> = o1_;
                        let _e548: vec2<f32> = resLo;
                        let _e555: vec2<f32> = o1_;
                        let _e557: vec2<f32> = o1_;
                        let _e560: vec2<f32> = resLo;
                        let _e566: vec2<f32> = o1_;
                        let _e568: vec2<f32> = o1_;
                        let _e571: vec2<f32> = resLo;
                        let _e578: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e566.x, _e568.y) / _e571), vec2(0f), vec2(1f)));
                        s00_ = _e578;
                        let _e579: vec2<f32> = o2_;
                        let _e581: vec2<f32> = o1_;
                        let _e584: vec2<f32> = resLo;
                        let _e590: vec2<f32> = o2_;
                        let _e592: vec2<f32> = o1_;
                        let _e595: vec2<f32> = resLo;
                        let _e602: vec2<f32> = o2_;
                        let _e604: vec2<f32> = o1_;
                        let _e607: vec2<f32> = resLo;
                        let _e613: vec2<f32> = o2_;
                        let _e615: vec2<f32> = o1_;
                        let _e618: vec2<f32> = resLo;
                        let _e625: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e613.x, _e615.y) / _e618), vec2(0f), vec2(1f)));
                        s10_ = _e625;
                        let _e626: vec2<f32> = o1_;
                        let _e628: vec2<f32> = o2_;
                        let _e631: vec2<f32> = resLo;
                        let _e637: vec2<f32> = o1_;
                        let _e639: vec2<f32> = o2_;
                        let _e642: vec2<f32> = resLo;
                        let _e649: vec2<f32> = o1_;
                        let _e651: vec2<f32> = o2_;
                        let _e654: vec2<f32> = resLo;
                        let _e660: vec2<f32> = o1_;
                        let _e662: vec2<f32> = o2_;
                        let _e665: vec2<f32> = resLo;
                        let _e672: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e660.x, _e662.y) / _e665), vec2(0f), vec2(1f)));
                        s01_ = _e672;
                        let _e673: vec2<f32> = o2_;
                        let _e675: vec2<f32> = o2_;
                        let _e678: vec2<f32> = resLo;
                        let _e684: vec2<f32> = o2_;
                        let _e686: vec2<f32> = o2_;
                        let _e689: vec2<f32> = resLo;
                        let _e696: vec2<f32> = o2_;
                        let _e698: vec2<f32> = o2_;
                        let _e701: vec2<f32> = resLo;
                        let _e707: vec2<f32> = o2_;
                        let _e709: vec2<f32> = o2_;
                        let _e712: vec2<f32> = resLo;
                        let _e719: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e707.x, _e709.y) / _e712), vec2(0f), vec2(1f)));
                        s11_ = _e719;
                    }
                } else {
                    let _e720: f32 = mLo;
                    if (_e720 < 3.5f) {
                        {
                            let _e723: vec2<f32> = o1_;
                            let _e725: vec2<f32> = o1_;
                            let _e728: vec2<f32> = resLo;
                            let _e734: vec2<f32> = o1_;
                            let _e736: vec2<f32> = o1_;
                            let _e739: vec2<f32> = resLo;
                            let _e746: vec2<f32> = o1_;
                            let _e748: vec2<f32> = o1_;
                            let _e751: vec2<f32> = resLo;
                            let _e757: vec2<f32> = o1_;
                            let _e759: vec2<f32> = o1_;
                            let _e762: vec2<f32> = resLo;
                            let _e769: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e757.x, _e759.y) / _e762), vec2(0f), vec2(1f)));
                            s00_ = _e769;
                            let _e770: vec2<f32> = o2_;
                            let _e772: vec2<f32> = o1_;
                            let _e775: vec2<f32> = resLo;
                            let _e781: vec2<f32> = o2_;
                            let _e783: vec2<f32> = o1_;
                            let _e786: vec2<f32> = resLo;
                            let _e793: vec2<f32> = o2_;
                            let _e795: vec2<f32> = o1_;
                            let _e798: vec2<f32> = resLo;
                            let _e804: vec2<f32> = o2_;
                            let _e806: vec2<f32> = o1_;
                            let _e809: vec2<f32> = resLo;
                            let _e816: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e804.x, _e806.y) / _e809), vec2(0f), vec2(1f)));
                            s10_ = _e816;
                            let _e817: vec2<f32> = o1_;
                            let _e819: vec2<f32> = o2_;
                            let _e822: vec2<f32> = resLo;
                            let _e828: vec2<f32> = o1_;
                            let _e830: vec2<f32> = o2_;
                            let _e833: vec2<f32> = resLo;
                            let _e840: vec2<f32> = o1_;
                            let _e842: vec2<f32> = o2_;
                            let _e845: vec2<f32> = resLo;
                            let _e851: vec2<f32> = o1_;
                            let _e853: vec2<f32> = o2_;
                            let _e856: vec2<f32> = resLo;
                            let _e863: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e851.x, _e853.y) / _e856), vec2(0f), vec2(1f)));
                            s01_ = _e863;
                            let _e864: vec2<f32> = o2_;
                            let _e866: vec2<f32> = o2_;
                            let _e869: vec2<f32> = resLo;
                            let _e875: vec2<f32> = o2_;
                            let _e877: vec2<f32> = o2_;
                            let _e880: vec2<f32> = resLo;
                            let _e887: vec2<f32> = o2_;
                            let _e889: vec2<f32> = o2_;
                            let _e892: vec2<f32> = resLo;
                            let _e898: vec2<f32> = o2_;
                            let _e900: vec2<f32> = o2_;
                            let _e903: vec2<f32> = resLo;
                            let _e910: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e898.x, _e900.y) / _e903), vec2(0f), vec2(1f)));
                            s11_ = _e910;
                        }
                    } else {
                        let _e911: f32 = mLo;
                        if (_e911 < 4.5f) {
                            {
                                let _e914: vec2<f32> = o1_;
                                let _e916: vec2<f32> = o1_;
                                let _e919: vec2<f32> = resLo;
                                let _e925: vec2<f32> = o1_;
                                let _e927: vec2<f32> = o1_;
                                let _e930: vec2<f32> = resLo;
                                let _e937: vec2<f32> = o1_;
                                let _e939: vec2<f32> = o1_;
                                let _e942: vec2<f32> = resLo;
                                let _e948: vec2<f32> = o1_;
                                let _e950: vec2<f32> = o1_;
                                let _e953: vec2<f32> = resLo;
                                let _e960: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e948.x, _e950.y) / _e953), vec2(0f), vec2(1f)));
                                s00_ = _e960;
                                let _e961: vec2<f32> = o2_;
                                let _e963: vec2<f32> = o1_;
                                let _e966: vec2<f32> = resLo;
                                let _e972: vec2<f32> = o2_;
                                let _e974: vec2<f32> = o1_;
                                let _e977: vec2<f32> = resLo;
                                let _e984: vec2<f32> = o2_;
                                let _e986: vec2<f32> = o1_;
                                let _e989: vec2<f32> = resLo;
                                let _e995: vec2<f32> = o2_;
                                let _e997: vec2<f32> = o1_;
                                let _e1000: vec2<f32> = resLo;
                                let _e1007: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e995.x, _e997.y) / _e1000), vec2(0f), vec2(1f)));
                                s10_ = _e1007;
                                let _e1008: vec2<f32> = o1_;
                                let _e1010: vec2<f32> = o2_;
                                let _e1013: vec2<f32> = resLo;
                                let _e1019: vec2<f32> = o1_;
                                let _e1021: vec2<f32> = o2_;
                                let _e1024: vec2<f32> = resLo;
                                let _e1031: vec2<f32> = o1_;
                                let _e1033: vec2<f32> = o2_;
                                let _e1036: vec2<f32> = resLo;
                                let _e1042: vec2<f32> = o1_;
                                let _e1044: vec2<f32> = o2_;
                                let _e1047: vec2<f32> = resLo;
                                let _e1054: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e1042.x, _e1044.y) / _e1047), vec2(0f), vec2(1f)));
                                s01_ = _e1054;
                                let _e1055: vec2<f32> = o2_;
                                let _e1057: vec2<f32> = o2_;
                                let _e1060: vec2<f32> = resLo;
                                let _e1066: vec2<f32> = o2_;
                                let _e1068: vec2<f32> = o2_;
                                let _e1071: vec2<f32> = resLo;
                                let _e1078: vec2<f32> = o2_;
                                let _e1080: vec2<f32> = o2_;
                                let _e1083: vec2<f32> = resLo;
                                let _e1089: vec2<f32> = o2_;
                                let _e1091: vec2<f32> = o2_;
                                let _e1094: vec2<f32> = resLo;
                                let _e1101: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e1089.x, _e1091.y) / _e1094), vec2(0f), vec2(1f)));
                                s11_ = _e1101;
                            }
                        } else {
                            let _e1102: f32 = mLo;
                            if (_e1102 < 5.5f) {
                                {
                                    let _e1105: vec2<f32> = o1_;
                                    let _e1107: vec2<f32> = o1_;
                                    let _e1110: vec2<f32> = resLo;
                                    let _e1116: vec2<f32> = o1_;
                                    let _e1118: vec2<f32> = o1_;
                                    let _e1121: vec2<f32> = resLo;
                                    let _e1128: vec2<f32> = o1_;
                                    let _e1130: vec2<f32> = o1_;
                                    let _e1133: vec2<f32> = resLo;
                                    let _e1139: vec2<f32> = o1_;
                                    let _e1141: vec2<f32> = o1_;
                                    let _e1144: vec2<f32> = resLo;
                                    let _e1151: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e1139.x, _e1141.y) / _e1144), vec2(0f), vec2(1f)));
                                    s00_ = _e1151;
                                    let _e1152: vec2<f32> = o2_;
                                    let _e1154: vec2<f32> = o1_;
                                    let _e1157: vec2<f32> = resLo;
                                    let _e1163: vec2<f32> = o2_;
                                    let _e1165: vec2<f32> = o1_;
                                    let _e1168: vec2<f32> = resLo;
                                    let _e1175: vec2<f32> = o2_;
                                    let _e1177: vec2<f32> = o1_;
                                    let _e1180: vec2<f32> = resLo;
                                    let _e1186: vec2<f32> = o2_;
                                    let _e1188: vec2<f32> = o1_;
                                    let _e1191: vec2<f32> = resLo;
                                    let _e1198: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e1186.x, _e1188.y) / _e1191), vec2(0f), vec2(1f)));
                                    s10_ = _e1198;
                                    let _e1199: vec2<f32> = o1_;
                                    let _e1201: vec2<f32> = o2_;
                                    let _e1204: vec2<f32> = resLo;
                                    let _e1210: vec2<f32> = o1_;
                                    let _e1212: vec2<f32> = o2_;
                                    let _e1215: vec2<f32> = resLo;
                                    let _e1222: vec2<f32> = o1_;
                                    let _e1224: vec2<f32> = o2_;
                                    let _e1227: vec2<f32> = resLo;
                                    let _e1233: vec2<f32> = o1_;
                                    let _e1235: vec2<f32> = o2_;
                                    let _e1238: vec2<f32> = resLo;
                                    let _e1245: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e1233.x, _e1235.y) / _e1238), vec2(0f), vec2(1f)));
                                    s01_ = _e1245;
                                    let _e1246: vec2<f32> = o2_;
                                    let _e1248: vec2<f32> = o2_;
                                    let _e1251: vec2<f32> = resLo;
                                    let _e1257: vec2<f32> = o2_;
                                    let _e1259: vec2<f32> = o2_;
                                    let _e1262: vec2<f32> = resLo;
                                    let _e1269: vec2<f32> = o2_;
                                    let _e1271: vec2<f32> = o2_;
                                    let _e1274: vec2<f32> = resLo;
                                    let _e1280: vec2<f32> = o2_;
                                    let _e1282: vec2<f32> = o2_;
                                    let _e1285: vec2<f32> = resLo;
                                    let _e1292: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e1280.x, _e1282.y) / _e1285), vec2(0f), vec2(1f)));
                                    s11_ = _e1292;
                                }
                            } else {
                                {
                                    let _e1293: vec2<f32> = o1_;
                                    let _e1295: vec2<f32> = o1_;
                                    let _e1298: vec2<f32> = resLo;
                                    let _e1304: vec2<f32> = o1_;
                                    let _e1306: vec2<f32> = o1_;
                                    let _e1309: vec2<f32> = resLo;
                                    let _e1316: vec2<f32> = o1_;
                                    let _e1318: vec2<f32> = o1_;
                                    let _e1321: vec2<f32> = resLo;
                                    let _e1327: vec2<f32> = o1_;
                                    let _e1329: vec2<f32> = o1_;
                                    let _e1332: vec2<f32> = resLo;
                                    let _e1339: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e1327.x, _e1329.y) / _e1332), vec2(0f), vec2(1f)));
                                    s00_ = _e1339;
                                    let _e1340: vec2<f32> = o2_;
                                    let _e1342: vec2<f32> = o1_;
                                    let _e1345: vec2<f32> = resLo;
                                    let _e1351: vec2<f32> = o2_;
                                    let _e1353: vec2<f32> = o1_;
                                    let _e1356: vec2<f32> = resLo;
                                    let _e1363: vec2<f32> = o2_;
                                    let _e1365: vec2<f32> = o1_;
                                    let _e1368: vec2<f32> = resLo;
                                    let _e1374: vec2<f32> = o2_;
                                    let _e1376: vec2<f32> = o1_;
                                    let _e1379: vec2<f32> = resLo;
                                    let _e1386: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e1374.x, _e1376.y) / _e1379), vec2(0f), vec2(1f)));
                                    s10_ = _e1386;
                                    let _e1387: vec2<f32> = o1_;
                                    let _e1389: vec2<f32> = o2_;
                                    let _e1392: vec2<f32> = resLo;
                                    let _e1398: vec2<f32> = o1_;
                                    let _e1400: vec2<f32> = o2_;
                                    let _e1403: vec2<f32> = resLo;
                                    let _e1410: vec2<f32> = o1_;
                                    let _e1412: vec2<f32> = o2_;
                                    let _e1415: vec2<f32> = resLo;
                                    let _e1421: vec2<f32> = o1_;
                                    let _e1423: vec2<f32> = o2_;
                                    let _e1426: vec2<f32> = resLo;
                                    let _e1433: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e1421.x, _e1423.y) / _e1426), vec2(0f), vec2(1f)));
                                    s01_ = _e1433;
                                    let _e1434: vec2<f32> = o2_;
                                    let _e1436: vec2<f32> = o2_;
                                    let _e1439: vec2<f32> = resLo;
                                    let _e1445: vec2<f32> = o2_;
                                    let _e1447: vec2<f32> = o2_;
                                    let _e1450: vec2<f32> = resLo;
                                    let _e1457: vec2<f32> = o2_;
                                    let _e1459: vec2<f32> = o2_;
                                    let _e1462: vec2<f32> = resLo;
                                    let _e1468: vec2<f32> = o2_;
                                    let _e1470: vec2<f32> = o2_;
                                    let _e1473: vec2<f32> = resLo;
                                    let _e1480: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e1468.x, _e1470.y) / _e1473), vec2(0f), vec2(1f)));
                                    s11_ = _e1480;
                                }
                            }
                        }
                    }
                }
            }
            let _e1481: vec2<f32> = w1_;
            let _e1483: vec2<f32> = w1_;
            let _e1486: vec4<f32> = s00_;
            let _e1488: vec2<f32> = w2_;
            let _e1490: vec2<f32> = w1_;
            let _e1493: vec4<f32> = s10_;
            let _e1496: vec2<f32> = w1_;
            let _e1498: vec2<f32> = w2_;
            let _e1501: vec4<f32> = s01_;
            let _e1504: vec2<f32> = w2_;
            let _e1506: vec2<f32> = w2_;
            let _e1509: vec4<f32> = s11_;
            clo = (((((_e1481.x * _e1483.y) * _e1486) + ((_e1488.x * _e1490.y) * _e1493)) + ((_e1496.x * _e1498.y) * _e1501)) + ((_e1504.x * _e1506.y) * _e1509));
        }
    }
    let _e1512: f32 = mLo;
    mHi = (_e1512 + 1f);
    let _e1520: f32 = mHi;
    scale = (1f / pow(2f, _e1520));
    let _e1523: vec2<f32> = coord;
    let _e1524: f32 = scale;
    dHi = ((_e1523 * _e1524) - vec2(0.5f));
    let _e1531: vec2<f32> = dHi;
    cHi_1 = floor(_e1531);
    let _e1534: vec2<f32> = cHi_1;
    let _e1535: vec2<f32> = dHi;
    xHi = ((_e1534 - _e1535) + vec2(1f));
    let _e1541: vec2<f32> = dHi;
    let _e1542: vec2<f32> = cHi_1;
    XHi = (_e1541 - _e1542);
    let _e1545: vec2<f32> = xHi;
    let _e1546: vec2<f32> = xHi;
    let _e1548: vec2<f32> = xHi;
    x3Hi = ((_e1545 * _e1546) * _e1548);
    let _e1552: vec2<f32> = xHi;
    let _e1554: vec2<f32> = xHi;
    let _e1557: vec2<f32> = xHi;
    coeffHi = ((((0.5f * _e1552) * _e1554) + (0.5f * _e1557)) + vec2(0.166667f));
    let _e1566: vec2<f32> = x3Hi;
    let _e1568: vec2<f32> = coeffHi;
    w1Hi = ((-0.333333f * _e1566) + _e1568);
    let _e1572: vec2<f32> = w1Hi;
    w2Hi = (vec2(1f) - _e1572);
    let _e1578: vec2<f32> = x3Hi;
    let _e1580: vec2<f32> = coeffHi;
    let _e1582: vec2<f32> = w1Hi;
    let _e1584: vec2<f32> = cHi_1;
    o1Hi = (((((-0.5f * _e1578) + _e1580) / _e1582) + _e1584) - vec2(0.5f));
    let _e1590: vec2<f32> = XHi;
    let _e1591: vec2<f32> = XHi;
    let _e1593: vec2<f32> = XHi;
    let _e1598: vec2<f32> = w2Hi;
    let _e1600: vec2<f32> = cHi_1;
    o2Hi = ((((((_e1590 * _e1591) * _e1593) / vec2(6f)) / _e1598) + _e1600) + vec2(1.5f));
    let _e1606: vec2<f32> = uEnlargedTextureSize;
    let _e1610: f32 = mHi;
    resHi = (_e1606 / vec2(pow(2f, _e1610)));
    let _e1619: f32 = mHi;
    if (_e1619 < 1.5f) {
        {
            let _e1622: vec2<f32> = o1Hi;
            let _e1624: vec2<f32> = o1Hi;
            let _e1627: vec2<f32> = resHi;
            let _e1633: vec2<f32> = o1Hi;
            let _e1635: vec2<f32> = o1Hi;
            let _e1638: vec2<f32> = resHi;
            let _e1645: vec2<f32> = o1Hi;
            let _e1647: vec2<f32> = o1Hi;
            let _e1650: vec2<f32> = resHi;
            let _e1656: vec2<f32> = o1Hi;
            let _e1658: vec2<f32> = o1Hi;
            let _e1661: vec2<f32> = resHi;
            let _e1668: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e1656.x, _e1658.y) / _e1661), vec2(0f), vec2(1f)));
            h00_ = _e1668;
            let _e1669: vec2<f32> = o2Hi;
            let _e1671: vec2<f32> = o1Hi;
            let _e1674: vec2<f32> = resHi;
            let _e1680: vec2<f32> = o2Hi;
            let _e1682: vec2<f32> = o1Hi;
            let _e1685: vec2<f32> = resHi;
            let _e1692: vec2<f32> = o2Hi;
            let _e1694: vec2<f32> = o1Hi;
            let _e1697: vec2<f32> = resHi;
            let _e1703: vec2<f32> = o2Hi;
            let _e1705: vec2<f32> = o1Hi;
            let _e1708: vec2<f32> = resHi;
            let _e1715: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e1703.x, _e1705.y) / _e1708), vec2(0f), vec2(1f)));
            h10_ = _e1715;
            let _e1716: vec2<f32> = o1Hi;
            let _e1718: vec2<f32> = o2Hi;
            let _e1721: vec2<f32> = resHi;
            let _e1727: vec2<f32> = o1Hi;
            let _e1729: vec2<f32> = o2Hi;
            let _e1732: vec2<f32> = resHi;
            let _e1739: vec2<f32> = o1Hi;
            let _e1741: vec2<f32> = o2Hi;
            let _e1744: vec2<f32> = resHi;
            let _e1750: vec2<f32> = o1Hi;
            let _e1752: vec2<f32> = o2Hi;
            let _e1755: vec2<f32> = resHi;
            let _e1762: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e1750.x, _e1752.y) / _e1755), vec2(0f), vec2(1f)));
            h01_ = _e1762;
            let _e1763: vec2<f32> = o2Hi;
            let _e1765: vec2<f32> = o2Hi;
            let _e1768: vec2<f32> = resHi;
            let _e1774: vec2<f32> = o2Hi;
            let _e1776: vec2<f32> = o2Hi;
            let _e1779: vec2<f32> = resHi;
            let _e1786: vec2<f32> = o2Hi;
            let _e1788: vec2<f32> = o2Hi;
            let _e1791: vec2<f32> = resHi;
            let _e1797: vec2<f32> = o2Hi;
            let _e1799: vec2<f32> = o2Hi;
            let _e1802: vec2<f32> = resHi;
            let _e1809: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e1797.x, _e1799.y) / _e1802), vec2(0f), vec2(1f)));
            h11_ = _e1809;
        }
    } else {
        let _e1810: f32 = mHi;
        if (_e1810 < 2.5f) {
            {
                let _e1813: vec2<f32> = o1Hi;
                let _e1815: vec2<f32> = o1Hi;
                let _e1818: vec2<f32> = resHi;
                let _e1824: vec2<f32> = o1Hi;
                let _e1826: vec2<f32> = o1Hi;
                let _e1829: vec2<f32> = resHi;
                let _e1836: vec2<f32> = o1Hi;
                let _e1838: vec2<f32> = o1Hi;
                let _e1841: vec2<f32> = resHi;
                let _e1847: vec2<f32> = o1Hi;
                let _e1849: vec2<f32> = o1Hi;
                let _e1852: vec2<f32> = resHi;
                let _e1859: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e1847.x, _e1849.y) / _e1852), vec2(0f), vec2(1f)));
                h00_ = _e1859;
                let _e1860: vec2<f32> = o2Hi;
                let _e1862: vec2<f32> = o1Hi;
                let _e1865: vec2<f32> = resHi;
                let _e1871: vec2<f32> = o2Hi;
                let _e1873: vec2<f32> = o1Hi;
                let _e1876: vec2<f32> = resHi;
                let _e1883: vec2<f32> = o2Hi;
                let _e1885: vec2<f32> = o1Hi;
                let _e1888: vec2<f32> = resHi;
                let _e1894: vec2<f32> = o2Hi;
                let _e1896: vec2<f32> = o1Hi;
                let _e1899: vec2<f32> = resHi;
                let _e1906: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e1894.x, _e1896.y) / _e1899), vec2(0f), vec2(1f)));
                h10_ = _e1906;
                let _e1907: vec2<f32> = o1Hi;
                let _e1909: vec2<f32> = o2Hi;
                let _e1912: vec2<f32> = resHi;
                let _e1918: vec2<f32> = o1Hi;
                let _e1920: vec2<f32> = o2Hi;
                let _e1923: vec2<f32> = resHi;
                let _e1930: vec2<f32> = o1Hi;
                let _e1932: vec2<f32> = o2Hi;
                let _e1935: vec2<f32> = resHi;
                let _e1941: vec2<f32> = o1Hi;
                let _e1943: vec2<f32> = o2Hi;
                let _e1946: vec2<f32> = resHi;
                let _e1953: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e1941.x, _e1943.y) / _e1946), vec2(0f), vec2(1f)));
                h01_ = _e1953;
                let _e1954: vec2<f32> = o2Hi;
                let _e1956: vec2<f32> = o2Hi;
                let _e1959: vec2<f32> = resHi;
                let _e1965: vec2<f32> = o2Hi;
                let _e1967: vec2<f32> = o2Hi;
                let _e1970: vec2<f32> = resHi;
                let _e1977: vec2<f32> = o2Hi;
                let _e1979: vec2<f32> = o2Hi;
                let _e1982: vec2<f32> = resHi;
                let _e1988: vec2<f32> = o2Hi;
                let _e1990: vec2<f32> = o2Hi;
                let _e1993: vec2<f32> = resHi;
                let _e2000: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e1988.x, _e1990.y) / _e1993), vec2(0f), vec2(1f)));
                h11_ = _e2000;
            }
        } else {
            let _e2001: f32 = mHi;
            if (_e2001 < 3.5f) {
                {
                    let _e2004: vec2<f32> = o1Hi;
                    let _e2006: vec2<f32> = o1Hi;
                    let _e2009: vec2<f32> = resHi;
                    let _e2015: vec2<f32> = o1Hi;
                    let _e2017: vec2<f32> = o1Hi;
                    let _e2020: vec2<f32> = resHi;
                    let _e2027: vec2<f32> = o1Hi;
                    let _e2029: vec2<f32> = o1Hi;
                    let _e2032: vec2<f32> = resHi;
                    let _e2038: vec2<f32> = o1Hi;
                    let _e2040: vec2<f32> = o1Hi;
                    let _e2043: vec2<f32> = resHi;
                    let _e2050: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e2038.x, _e2040.y) / _e2043), vec2(0f), vec2(1f)));
                    h00_ = _e2050;
                    let _e2051: vec2<f32> = o2Hi;
                    let _e2053: vec2<f32> = o1Hi;
                    let _e2056: vec2<f32> = resHi;
                    let _e2062: vec2<f32> = o2Hi;
                    let _e2064: vec2<f32> = o1Hi;
                    let _e2067: vec2<f32> = resHi;
                    let _e2074: vec2<f32> = o2Hi;
                    let _e2076: vec2<f32> = o1Hi;
                    let _e2079: vec2<f32> = resHi;
                    let _e2085: vec2<f32> = o2Hi;
                    let _e2087: vec2<f32> = o1Hi;
                    let _e2090: vec2<f32> = resHi;
                    let _e2097: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e2085.x, _e2087.y) / _e2090), vec2(0f), vec2(1f)));
                    h10_ = _e2097;
                    let _e2098: vec2<f32> = o1Hi;
                    let _e2100: vec2<f32> = o2Hi;
                    let _e2103: vec2<f32> = resHi;
                    let _e2109: vec2<f32> = o1Hi;
                    let _e2111: vec2<f32> = o2Hi;
                    let _e2114: vec2<f32> = resHi;
                    let _e2121: vec2<f32> = o1Hi;
                    let _e2123: vec2<f32> = o2Hi;
                    let _e2126: vec2<f32> = resHi;
                    let _e2132: vec2<f32> = o1Hi;
                    let _e2134: vec2<f32> = o2Hi;
                    let _e2137: vec2<f32> = resHi;
                    let _e2144: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e2132.x, _e2134.y) / _e2137), vec2(0f), vec2(1f)));
                    h01_ = _e2144;
                    let _e2145: vec2<f32> = o2Hi;
                    let _e2147: vec2<f32> = o2Hi;
                    let _e2150: vec2<f32> = resHi;
                    let _e2156: vec2<f32> = o2Hi;
                    let _e2158: vec2<f32> = o2Hi;
                    let _e2161: vec2<f32> = resHi;
                    let _e2168: vec2<f32> = o2Hi;
                    let _e2170: vec2<f32> = o2Hi;
                    let _e2173: vec2<f32> = resHi;
                    let _e2179: vec2<f32> = o2Hi;
                    let _e2181: vec2<f32> = o2Hi;
                    let _e2184: vec2<f32> = resHi;
                    let _e2191: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e2179.x, _e2181.y) / _e2184), vec2(0f), vec2(1f)));
                    h11_ = _e2191;
                }
            } else {
                let _e2192: f32 = mHi;
                if (_e2192 < 4.5f) {
                    {
                        let _e2195: vec2<f32> = o1Hi;
                        let _e2197: vec2<f32> = o1Hi;
                        let _e2200: vec2<f32> = resHi;
                        let _e2206: vec2<f32> = o1Hi;
                        let _e2208: vec2<f32> = o1Hi;
                        let _e2211: vec2<f32> = resHi;
                        let _e2218: vec2<f32> = o1Hi;
                        let _e2220: vec2<f32> = o1Hi;
                        let _e2223: vec2<f32> = resHi;
                        let _e2229: vec2<f32> = o1Hi;
                        let _e2231: vec2<f32> = o1Hi;
                        let _e2234: vec2<f32> = resHi;
                        let _e2241: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e2229.x, _e2231.y) / _e2234), vec2(0f), vec2(1f)));
                        h00_ = _e2241;
                        let _e2242: vec2<f32> = o2Hi;
                        let _e2244: vec2<f32> = o1Hi;
                        let _e2247: vec2<f32> = resHi;
                        let _e2253: vec2<f32> = o2Hi;
                        let _e2255: vec2<f32> = o1Hi;
                        let _e2258: vec2<f32> = resHi;
                        let _e2265: vec2<f32> = o2Hi;
                        let _e2267: vec2<f32> = o1Hi;
                        let _e2270: vec2<f32> = resHi;
                        let _e2276: vec2<f32> = o2Hi;
                        let _e2278: vec2<f32> = o1Hi;
                        let _e2281: vec2<f32> = resHi;
                        let _e2288: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e2276.x, _e2278.y) / _e2281), vec2(0f), vec2(1f)));
                        h10_ = _e2288;
                        let _e2289: vec2<f32> = o1Hi;
                        let _e2291: vec2<f32> = o2Hi;
                        let _e2294: vec2<f32> = resHi;
                        let _e2300: vec2<f32> = o1Hi;
                        let _e2302: vec2<f32> = o2Hi;
                        let _e2305: vec2<f32> = resHi;
                        let _e2312: vec2<f32> = o1Hi;
                        let _e2314: vec2<f32> = o2Hi;
                        let _e2317: vec2<f32> = resHi;
                        let _e2323: vec2<f32> = o1Hi;
                        let _e2325: vec2<f32> = o2Hi;
                        let _e2328: vec2<f32> = resHi;
                        let _e2335: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e2323.x, _e2325.y) / _e2328), vec2(0f), vec2(1f)));
                        h01_ = _e2335;
                        let _e2336: vec2<f32> = o2Hi;
                        let _e2338: vec2<f32> = o2Hi;
                        let _e2341: vec2<f32> = resHi;
                        let _e2347: vec2<f32> = o2Hi;
                        let _e2349: vec2<f32> = o2Hi;
                        let _e2352: vec2<f32> = resHi;
                        let _e2359: vec2<f32> = o2Hi;
                        let _e2361: vec2<f32> = o2Hi;
                        let _e2364: vec2<f32> = resHi;
                        let _e2370: vec2<f32> = o2Hi;
                        let _e2372: vec2<f32> = o2Hi;
                        let _e2375: vec2<f32> = resHi;
                        let _e2382: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e2370.x, _e2372.y) / _e2375), vec2(0f), vec2(1f)));
                        h11_ = _e2382;
                    }
                } else {
                    let _e2383: f32 = mHi;
                    if (_e2383 < 5.5f) {
                        {
                            let _e2386: vec2<f32> = o1Hi;
                            let _e2388: vec2<f32> = o1Hi;
                            let _e2391: vec2<f32> = resHi;
                            let _e2397: vec2<f32> = o1Hi;
                            let _e2399: vec2<f32> = o1Hi;
                            let _e2402: vec2<f32> = resHi;
                            let _e2409: vec2<f32> = o1Hi;
                            let _e2411: vec2<f32> = o1Hi;
                            let _e2414: vec2<f32> = resHi;
                            let _e2420: vec2<f32> = o1Hi;
                            let _e2422: vec2<f32> = o1Hi;
                            let _e2425: vec2<f32> = resHi;
                            let _e2432: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e2420.x, _e2422.y) / _e2425), vec2(0f), vec2(1f)));
                            h00_ = _e2432;
                            let _e2433: vec2<f32> = o2Hi;
                            let _e2435: vec2<f32> = o1Hi;
                            let _e2438: vec2<f32> = resHi;
                            let _e2444: vec2<f32> = o2Hi;
                            let _e2446: vec2<f32> = o1Hi;
                            let _e2449: vec2<f32> = resHi;
                            let _e2456: vec2<f32> = o2Hi;
                            let _e2458: vec2<f32> = o1Hi;
                            let _e2461: vec2<f32> = resHi;
                            let _e2467: vec2<f32> = o2Hi;
                            let _e2469: vec2<f32> = o1Hi;
                            let _e2472: vec2<f32> = resHi;
                            let _e2479: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e2467.x, _e2469.y) / _e2472), vec2(0f), vec2(1f)));
                            h10_ = _e2479;
                            let _e2480: vec2<f32> = o1Hi;
                            let _e2482: vec2<f32> = o2Hi;
                            let _e2485: vec2<f32> = resHi;
                            let _e2491: vec2<f32> = o1Hi;
                            let _e2493: vec2<f32> = o2Hi;
                            let _e2496: vec2<f32> = resHi;
                            let _e2503: vec2<f32> = o1Hi;
                            let _e2505: vec2<f32> = o2Hi;
                            let _e2508: vec2<f32> = resHi;
                            let _e2514: vec2<f32> = o1Hi;
                            let _e2516: vec2<f32> = o2Hi;
                            let _e2519: vec2<f32> = resHi;
                            let _e2526: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e2514.x, _e2516.y) / _e2519), vec2(0f), vec2(1f)));
                            h01_ = _e2526;
                            let _e2527: vec2<f32> = o2Hi;
                            let _e2529: vec2<f32> = o2Hi;
                            let _e2532: vec2<f32> = resHi;
                            let _e2538: vec2<f32> = o2Hi;
                            let _e2540: vec2<f32> = o2Hi;
                            let _e2543: vec2<f32> = resHi;
                            let _e2550: vec2<f32> = o2Hi;
                            let _e2552: vec2<f32> = o2Hi;
                            let _e2555: vec2<f32> = resHi;
                            let _e2561: vec2<f32> = o2Hi;
                            let _e2563: vec2<f32> = o2Hi;
                            let _e2566: vec2<f32> = resHi;
                            let _e2573: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e2561.x, _e2563.y) / _e2566), vec2(0f), vec2(1f)));
                            h11_ = _e2573;
                        }
                    } else {
                        let _e2574: f32 = mHi;
                        if (_e2574 < 6.5f) {
                            {
                                let _e2577: vec2<f32> = o1Hi;
                                let _e2579: vec2<f32> = o1Hi;
                                let _e2582: vec2<f32> = resHi;
                                let _e2588: vec2<f32> = o1Hi;
                                let _e2590: vec2<f32> = o1Hi;
                                let _e2593: vec2<f32> = resHi;
                                let _e2600: vec2<f32> = o1Hi;
                                let _e2602: vec2<f32> = o1Hi;
                                let _e2605: vec2<f32> = resHi;
                                let _e2611: vec2<f32> = o1Hi;
                                let _e2613: vec2<f32> = o1Hi;
                                let _e2616: vec2<f32> = resHi;
                                let _e2623: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2611.x, _e2613.y) / _e2616), vec2(0f), vec2(1f)));
                                h00_ = _e2623;
                                let _e2624: vec2<f32> = o2Hi;
                                let _e2626: vec2<f32> = o1Hi;
                                let _e2629: vec2<f32> = resHi;
                                let _e2635: vec2<f32> = o2Hi;
                                let _e2637: vec2<f32> = o1Hi;
                                let _e2640: vec2<f32> = resHi;
                                let _e2647: vec2<f32> = o2Hi;
                                let _e2649: vec2<f32> = o1Hi;
                                let _e2652: vec2<f32> = resHi;
                                let _e2658: vec2<f32> = o2Hi;
                                let _e2660: vec2<f32> = o1Hi;
                                let _e2663: vec2<f32> = resHi;
                                let _e2670: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2658.x, _e2660.y) / _e2663), vec2(0f), vec2(1f)));
                                h10_ = _e2670;
                                let _e2671: vec2<f32> = o1Hi;
                                let _e2673: vec2<f32> = o2Hi;
                                let _e2676: vec2<f32> = resHi;
                                let _e2682: vec2<f32> = o1Hi;
                                let _e2684: vec2<f32> = o2Hi;
                                let _e2687: vec2<f32> = resHi;
                                let _e2694: vec2<f32> = o1Hi;
                                let _e2696: vec2<f32> = o2Hi;
                                let _e2699: vec2<f32> = resHi;
                                let _e2705: vec2<f32> = o1Hi;
                                let _e2707: vec2<f32> = o2Hi;
                                let _e2710: vec2<f32> = resHi;
                                let _e2717: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2705.x, _e2707.y) / _e2710), vec2(0f), vec2(1f)));
                                h01_ = _e2717;
                                let _e2718: vec2<f32> = o2Hi;
                                let _e2720: vec2<f32> = o2Hi;
                                let _e2723: vec2<f32> = resHi;
                                let _e2729: vec2<f32> = o2Hi;
                                let _e2731: vec2<f32> = o2Hi;
                                let _e2734: vec2<f32> = resHi;
                                let _e2741: vec2<f32> = o2Hi;
                                let _e2743: vec2<f32> = o2Hi;
                                let _e2746: vec2<f32> = resHi;
                                let _e2752: vec2<f32> = o2Hi;
                                let _e2754: vec2<f32> = o2Hi;
                                let _e2757: vec2<f32> = resHi;
                                let _e2764: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2752.x, _e2754.y) / _e2757), vec2(0f), vec2(1f)));
                                h11_ = _e2764;
                            }
                        } else {
                            {
                                let _e2765: vec2<f32> = o1Hi;
                                let _e2767: vec2<f32> = o1Hi;
                                let _e2770: vec2<f32> = resHi;
                                let _e2776: vec2<f32> = o1Hi;
                                let _e2778: vec2<f32> = o1Hi;
                                let _e2781: vec2<f32> = resHi;
                                let _e2788: vec2<f32> = o1Hi;
                                let _e2790: vec2<f32> = o1Hi;
                                let _e2793: vec2<f32> = resHi;
                                let _e2799: vec2<f32> = o1Hi;
                                let _e2801: vec2<f32> = o1Hi;
                                let _e2804: vec2<f32> = resHi;
                                let _e2811: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2799.x, _e2801.y) / _e2804), vec2(0f), vec2(1f)));
                                h00_ = _e2811;
                                let _e2812: vec2<f32> = o2Hi;
                                let _e2814: vec2<f32> = o1Hi;
                                let _e2817: vec2<f32> = resHi;
                                let _e2823: vec2<f32> = o2Hi;
                                let _e2825: vec2<f32> = o1Hi;
                                let _e2828: vec2<f32> = resHi;
                                let _e2835: vec2<f32> = o2Hi;
                                let _e2837: vec2<f32> = o1Hi;
                                let _e2840: vec2<f32> = resHi;
                                let _e2846: vec2<f32> = o2Hi;
                                let _e2848: vec2<f32> = o1Hi;
                                let _e2851: vec2<f32> = resHi;
                                let _e2858: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2846.x, _e2848.y) / _e2851), vec2(0f), vec2(1f)));
                                h10_ = _e2858;
                                let _e2859: vec2<f32> = o1Hi;
                                let _e2861: vec2<f32> = o2Hi;
                                let _e2864: vec2<f32> = resHi;
                                let _e2870: vec2<f32> = o1Hi;
                                let _e2872: vec2<f32> = o2Hi;
                                let _e2875: vec2<f32> = resHi;
                                let _e2882: vec2<f32> = o1Hi;
                                let _e2884: vec2<f32> = o2Hi;
                                let _e2887: vec2<f32> = resHi;
                                let _e2893: vec2<f32> = o1Hi;
                                let _e2895: vec2<f32> = o2Hi;
                                let _e2898: vec2<f32> = resHi;
                                let _e2905: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2893.x, _e2895.y) / _e2898), vec2(0f), vec2(1f)));
                                h01_ = _e2905;
                                let _e2906: vec2<f32> = o2Hi;
                                let _e2908: vec2<f32> = o2Hi;
                                let _e2911: vec2<f32> = resHi;
                                let _e2917: vec2<f32> = o2Hi;
                                let _e2919: vec2<f32> = o2Hi;
                                let _e2922: vec2<f32> = resHi;
                                let _e2929: vec2<f32> = o2Hi;
                                let _e2931: vec2<f32> = o2Hi;
                                let _e2934: vec2<f32> = resHi;
                                let _e2940: vec2<f32> = o2Hi;
                                let _e2942: vec2<f32> = o2Hi;
                                let _e2945: vec2<f32> = resHi;
                                let _e2952: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2940.x, _e2942.y) / _e2945), vec2(0f), vec2(1f)));
                                h11_ = _e2952;
                            }
                        }
                    }
                }
            }
        }
    }
    let _e2953: vec2<f32> = w1Hi;
    let _e2955: vec2<f32> = w1Hi;
    let _e2958: vec4<f32> = h00_;
    let _e2960: vec2<f32> = w2Hi;
    let _e2962: vec2<f32> = w1Hi;
    let _e2965: vec4<f32> = h10_;
    let _e2968: vec2<f32> = w1Hi;
    let _e2970: vec2<f32> = w2Hi;
    let _e2973: vec4<f32> = h01_;
    let _e2976: vec2<f32> = w2Hi;
    let _e2978: vec2<f32> = w2Hi;
    let _e2981: vec4<f32> = h11_;
    cHi = (((((_e2953.x * _e2955.y) * _e2958) + ((_e2960.x * _e2962.y) * _e2965)) + ((_e2968.x * _e2970.y) * _e2973)) + ((_e2976.x * _e2978.y) * _e2981));
    let _e2986: f32 = m_1;
    let _e2987: f32 = mLo;
    let _e2989: vec4<f32> = clo;
    let _e2990: vec4<f32> = cHi;
    let _e2991: f32 = m_1;
    let _e2992: f32 = mLo;
    output = mix(_e2989, _e2990, vec4((_e2991 - _e2992)));
    let _e2996: vec4<f32> = output;
    return _e2996;
}

fn sample_pass_Downsample_mip1_(uv_in: vec2f) -> vec4f {
    // NOTE: PassTexture Y flip for bottom-left UV convention.
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip1, pass_samp_Downsample_mip1, uv_flipped);
}

fn sample_pass_Downsample_mip2_(uv_in: vec2f) -> vec4f {
    // NOTE: PassTexture Y flip for bottom-left UV convention.
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip2, pass_samp_Downsample_mip2, uv_flipped);
}

fn sample_pass_Downsample_mip3_(uv_in: vec2f) -> vec4f {
    // NOTE: PassTexture Y flip for bottom-left UV convention.
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip3, pass_samp_Downsample_mip3, uv_flipped);
}

fn sample_pass_Downsample_mip4_(uv_in: vec2f) -> vec4f {
    // NOTE: PassTexture Y flip for bottom-left UV convention.
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip4, pass_samp_Downsample_mip4, uv_flipped);
}

fn sample_pass_Downsample_mip5_(uv_in: vec2f) -> vec4f {
    // NOTE: PassTexture Y flip for bottom-left UV convention.
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip5, pass_samp_Downsample_mip5, uv_flipped);
}

fn sample_pass_Downsample_mip6_(uv_in: vec2f) -> vec4f {
    // NOTE: PassTexture Y flip for bottom-left UV convention.
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip6, pass_samp_Downsample_mip6, uv_flipped);
}

fn sample_pass_RenderPass_mip0_(uv_in: vec2f) -> vec4f {
    // NOTE: PassTexture Y flip for bottom-left UV convention.
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_RenderPass_mip0, pass_samp_RenderPass_mip0, uv_flipped);
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
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_calculateMaskLinear_out: f32;
    {
        let xy = in.local_px;
        var output: f32;
        output = mc_MathClosure_calculateMaskLinear(in.uv, xy);
        mc_MathClosure_calculateMaskLinear_out = output;
    }
    var mc_MathClosure_fragment_out: vec4f;
    {
        let xy = in.local_px;
        let m = mc_MathClosure_calculateMaskLinear_out;
        var output: vec4f;
        output = mc_MathClosure_fragment(in.uv, xy, m);
        mc_MathClosure_fragment_out = output;
    }
    return mc_MathClosure_fragment_out;
}
