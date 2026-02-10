
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
    // Node: FloatInput_95
    node_FloatInput_95_0ec7eb20: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_RenderPass_85: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_RenderPass_85: sampler;

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
fn mc_MathClosure_calculateMaskLinear(uv: vec2<f32>, xy: vec2<f32>, start_x: f32) -> f32 {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var start_x_1: f32;
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
    start_x_1 = start_x;
    let _e17: f32 = textureHeight;
    pEnd = vec3<f32>(0f, _e17, 0f);
    let _e21: vec3<f32> = pEnd;
    let _e23: vec3<f32> = pStart;
    qBase = (_e21.xy - _e23.xy);
    let _e29: vec2<f32> = qBase;
    let _e30: vec2<f32> = qBase;
    md = dot(_e29, _e30);
    let _e33: vec2<f32> = xy_1;
    let _e34: vec3<f32> = pStart;
    q = (_e33 - _e34.xy);
    let _e40: vec2<f32> = q;
    let _e41: vec2<f32> = qBase;
    p = dot(_e40, _e41);
    let _e47: f32 = md;
    let _e49: f32 = p;
    m = smoothstep(_e47, 0f, _e49);
    let _e52: vec3<f32> = pEnd;
    let _e54: vec3<f32> = pStart;
    let _e56: vec3<f32> = pEnd;
    let _e59: f32 = m;
    m = (_e52.z + ((_e54.z - _e56.z) * _e59));
    let _e62: f32 = m;
    let _e65: f32 = m;
    m = log2((_e65 * 1.333333f));
    let _e72: f32 = m;
    output = clamp(_e72, 0f, 6f);
    let _e76: f32 = output;
    return _e76;
}

fn mc_MathClosure_fragment(uv: vec2<f32>, xy: vec2<f32>, m: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var m_1: f32;
    var output: vec4<f32> = vec4(0f);
    var coord: vec2<f32>;
    var uEnlargedTextureSize: vec2<f32> = vec2<f32>(1152f, 2496f);
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
            let _e37: vec2<f32> = p0_;
            let _e41: vec2<f32> = p0_;
            f0_ = fract((_e41 + vec2(0.5f)));
            let _e47: vec2<i32> = i0_;
            let _e53: vec2<f32> = res0_;
            let _e59: vec2<i32> = i0_;
            let _e65: vec2<f32> = res0_;
            uv00_ = clamp((vec2<f32>((vec2<f32>(_e59) + vec2(0.5f))) / _e65), vec2(0f), vec2(1f));
            let _e73: vec2<i32> = i0_;
            let _e86: vec2<f32> = res0_;
            let _e92: vec2<i32> = i0_;
            let _e105: vec2<f32> = res0_;
            uv10_ = clamp((vec2<f32>(((vec2<f32>(_e92) + vec2(0.5f)) + vec2<f32>(1f, 0f))) / _e105), vec2(0f), vec2(1f));
            let _e113: vec2<i32> = i0_;
            let _e126: vec2<f32> = res0_;
            let _e132: vec2<i32> = i0_;
            let _e145: vec2<f32> = res0_;
            uv01_ = clamp((vec2<f32>(((vec2<f32>(_e132) + vec2(0.5f)) + vec2<f32>(0f, 1f))) / _e145), vec2(0f), vec2(1f));
            let _e153: vec2<i32> = i0_;
            let _e166: vec2<f32> = res0_;
            let _e172: vec2<i32> = i0_;
            let _e185: vec2<f32> = res0_;
            uv11_ = clamp((vec2<f32>(((vec2<f32>(_e172) + vec2(0.5f)) + vec2<f32>(1f, 1f))) / _e185), vec2(0f), vec2(1f));
            let _e194: vec2<f32> = uv00_;
            let _e195: vec4<f32> = sample_pass_RenderPass_85_(_e194);
            c00_ = _e195;
            let _e198: vec2<f32> = uv10_;
            let _e199: vec4<f32> = sample_pass_RenderPass_85_(_e198);
            c10_ = _e199;
            let _e202: vec2<f32> = uv01_;
            let _e203: vec4<f32> = sample_pass_RenderPass_85_(_e202);
            c01_ = _e203;
            let _e206: vec2<f32> = uv11_;
            let _e207: vec4<f32> = sample_pass_RenderPass_85_(_e206);
            c11_ = _e207;
            let _e211: vec2<f32> = f0_;
            let _e213: vec4<f32> = c00_;
            let _e214: vec4<f32> = c10_;
            let _e215: vec2<f32> = f0_;
            let _e221: vec2<f32> = f0_;
            let _e223: vec4<f32> = c01_;
            let _e224: vec4<f32> = c11_;
            let _e225: vec2<f32> = f0_;
            let _e229: vec2<f32> = f0_;
            let _e233: vec2<f32> = f0_;
            let _e235: vec4<f32> = c00_;
            let _e236: vec4<f32> = c10_;
            let _e237: vec2<f32> = f0_;
            let _e243: vec2<f32> = f0_;
            let _e245: vec4<f32> = c01_;
            let _e246: vec4<f32> = c11_;
            let _e247: vec2<f32> = f0_;
            let _e251: vec2<f32> = f0_;
            clo = mix(mix(_e235, _e236, vec4(_e237.x)), mix(_e245, _e246, vec4(_e247.x)), vec4(_e251.y));
        }
    } else {
        {
            let _e259: f32 = mLo;
            scale = (1f / pow(2f, _e259));
            let _e262: vec2<f32> = coord;
            let _e263: f32 = scale;
            d = ((_e262 * _e263) - vec2(0.5f));
            let _e270: vec2<f32> = d;
            c = floor(_e270);
            let _e273: vec2<f32> = c;
            let _e274: vec2<f32> = d;
            x = ((_e273 - _e274) + vec2(1f));
            let _e280: vec2<f32> = d;
            let _e281: vec2<f32> = c;
            X = (_e280 - _e281);
            let _e284: vec2<f32> = x;
            let _e285: vec2<f32> = x;
            let _e287: vec2<f32> = x;
            x3_ = ((_e284 * _e285) * _e287);
            let _e291: vec2<f32> = x;
            let _e293: vec2<f32> = x;
            let _e296: vec2<f32> = x;
            coeff = ((((0.5f * _e291) * _e293) + (0.5f * _e296)) + vec2(0.166667f));
            let _e305: vec2<f32> = x3_;
            let _e307: vec2<f32> = coeff;
            w1_ = ((-0.333333f * _e305) + _e307);
            let _e311: vec2<f32> = w1_;
            w2_ = (vec2(1f) - _e311);
            let _e317: vec2<f32> = x3_;
            let _e319: vec2<f32> = coeff;
            let _e321: vec2<f32> = w1_;
            let _e323: vec2<f32> = c;
            o1_ = (((((-0.5f * _e317) + _e319) / _e321) + _e323) - vec2(0.5f));
            let _e329: vec2<f32> = X;
            let _e330: vec2<f32> = X;
            let _e332: vec2<f32> = X;
            let _e337: vec2<f32> = w2_;
            let _e339: vec2<f32> = c;
            o2_ = ((((((_e329 * _e330) * _e332) / vec2(6f)) / _e337) + _e339) + vec2(1.5f));
            let _e345: vec2<f32> = uEnlargedTextureSize;
            let _e349: f32 = mLo;
            resLo = (_e345 / vec2(pow(2f, _e349)));
            let _e354: vec2<f32> = o1_;
            let _e356: vec2<f32> = o1_;
            p_o1o1_ = (vec2<f32>(_e354.x, _e356.y) - vec2(0.5f));
            let _e363: vec2<f32> = o2_;
            let _e365: vec2<f32> = o1_;
            p_o2o1_ = (vec2<f32>(_e363.x, _e365.y) - vec2(0.5f));
            let _e372: vec2<f32> = o1_;
            let _e374: vec2<f32> = o2_;
            p_o1o2_ = (vec2<f32>(_e372.x, _e374.y) - vec2(0.5f));
            let _e381: vec2<f32> = o2_;
            let _e383: vec2<f32> = o2_;
            p_o2o2_ = (vec2<f32>(_e381.x, _e383.y) - vec2(0.5f));
            let _e394: f32 = mLo;
            if (_e394 < 1.5f) {
                {
                    let _e397: vec2<f32> = o1_;
                    let _e399: vec2<f32> = o1_;
                    let _e402: vec2<f32> = resLo;
                    let _e408: vec2<f32> = o1_;
                    let _e410: vec2<f32> = o1_;
                    let _e413: vec2<f32> = resLo;
                    let _e420: vec2<f32> = o1_;
                    let _e422: vec2<f32> = o1_;
                    let _e425: vec2<f32> = resLo;
                    let _e431: vec2<f32> = o1_;
                    let _e433: vec2<f32> = o1_;
                    let _e436: vec2<f32> = resLo;
                    let _e443: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e431.x, _e433.y) / _e436), vec2(0f), vec2(1f)));
                    s00_ = _e443;
                    let _e444: vec2<f32> = o2_;
                    let _e446: vec2<f32> = o1_;
                    let _e449: vec2<f32> = resLo;
                    let _e455: vec2<f32> = o2_;
                    let _e457: vec2<f32> = o1_;
                    let _e460: vec2<f32> = resLo;
                    let _e467: vec2<f32> = o2_;
                    let _e469: vec2<f32> = o1_;
                    let _e472: vec2<f32> = resLo;
                    let _e478: vec2<f32> = o2_;
                    let _e480: vec2<f32> = o1_;
                    let _e483: vec2<f32> = resLo;
                    let _e490: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e478.x, _e480.y) / _e483), vec2(0f), vec2(1f)));
                    s10_ = _e490;
                    let _e491: vec2<f32> = o1_;
                    let _e493: vec2<f32> = o2_;
                    let _e496: vec2<f32> = resLo;
                    let _e502: vec2<f32> = o1_;
                    let _e504: vec2<f32> = o2_;
                    let _e507: vec2<f32> = resLo;
                    let _e514: vec2<f32> = o1_;
                    let _e516: vec2<f32> = o2_;
                    let _e519: vec2<f32> = resLo;
                    let _e525: vec2<f32> = o1_;
                    let _e527: vec2<f32> = o2_;
                    let _e530: vec2<f32> = resLo;
                    let _e537: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e525.x, _e527.y) / _e530), vec2(0f), vec2(1f)));
                    s01_ = _e537;
                    let _e538: vec2<f32> = o2_;
                    let _e540: vec2<f32> = o2_;
                    let _e543: vec2<f32> = resLo;
                    let _e549: vec2<f32> = o2_;
                    let _e551: vec2<f32> = o2_;
                    let _e554: vec2<f32> = resLo;
                    let _e561: vec2<f32> = o2_;
                    let _e563: vec2<f32> = o2_;
                    let _e566: vec2<f32> = resLo;
                    let _e572: vec2<f32> = o2_;
                    let _e574: vec2<f32> = o2_;
                    let _e577: vec2<f32> = resLo;
                    let _e584: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e572.x, _e574.y) / _e577), vec2(0f), vec2(1f)));
                    s11_ = _e584;
                }
            } else {
                let _e585: f32 = mLo;
                if (_e585 < 2.5f) {
                    {
                        let _e588: vec2<f32> = o1_;
                        let _e590: vec2<f32> = o1_;
                        let _e593: vec2<f32> = resLo;
                        let _e599: vec2<f32> = o1_;
                        let _e601: vec2<f32> = o1_;
                        let _e604: vec2<f32> = resLo;
                        let _e611: vec2<f32> = o1_;
                        let _e613: vec2<f32> = o1_;
                        let _e616: vec2<f32> = resLo;
                        let _e622: vec2<f32> = o1_;
                        let _e624: vec2<f32> = o1_;
                        let _e627: vec2<f32> = resLo;
                        let _e634: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e622.x, _e624.y) / _e627), vec2(0f), vec2(1f)));
                        s00_ = _e634;
                        let _e635: vec2<f32> = o2_;
                        let _e637: vec2<f32> = o1_;
                        let _e640: vec2<f32> = resLo;
                        let _e646: vec2<f32> = o2_;
                        let _e648: vec2<f32> = o1_;
                        let _e651: vec2<f32> = resLo;
                        let _e658: vec2<f32> = o2_;
                        let _e660: vec2<f32> = o1_;
                        let _e663: vec2<f32> = resLo;
                        let _e669: vec2<f32> = o2_;
                        let _e671: vec2<f32> = o1_;
                        let _e674: vec2<f32> = resLo;
                        let _e681: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e669.x, _e671.y) / _e674), vec2(0f), vec2(1f)));
                        s10_ = _e681;
                        let _e682: vec2<f32> = o1_;
                        let _e684: vec2<f32> = o2_;
                        let _e687: vec2<f32> = resLo;
                        let _e693: vec2<f32> = o1_;
                        let _e695: vec2<f32> = o2_;
                        let _e698: vec2<f32> = resLo;
                        let _e705: vec2<f32> = o1_;
                        let _e707: vec2<f32> = o2_;
                        let _e710: vec2<f32> = resLo;
                        let _e716: vec2<f32> = o1_;
                        let _e718: vec2<f32> = o2_;
                        let _e721: vec2<f32> = resLo;
                        let _e728: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e716.x, _e718.y) / _e721), vec2(0f), vec2(1f)));
                        s01_ = _e728;
                        let _e729: vec2<f32> = o2_;
                        let _e731: vec2<f32> = o2_;
                        let _e734: vec2<f32> = resLo;
                        let _e740: vec2<f32> = o2_;
                        let _e742: vec2<f32> = o2_;
                        let _e745: vec2<f32> = resLo;
                        let _e752: vec2<f32> = o2_;
                        let _e754: vec2<f32> = o2_;
                        let _e757: vec2<f32> = resLo;
                        let _e763: vec2<f32> = o2_;
                        let _e765: vec2<f32> = o2_;
                        let _e768: vec2<f32> = resLo;
                        let _e775: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e763.x, _e765.y) / _e768), vec2(0f), vec2(1f)));
                        s11_ = _e775;
                    }
                } else {
                    let _e776: f32 = mLo;
                    if (_e776 < 3.5f) {
                        {
                            let _e779: vec2<f32> = o1_;
                            let _e781: vec2<f32> = o1_;
                            let _e784: vec2<f32> = resLo;
                            let _e790: vec2<f32> = o1_;
                            let _e792: vec2<f32> = o1_;
                            let _e795: vec2<f32> = resLo;
                            let _e802: vec2<f32> = o1_;
                            let _e804: vec2<f32> = o1_;
                            let _e807: vec2<f32> = resLo;
                            let _e813: vec2<f32> = o1_;
                            let _e815: vec2<f32> = o1_;
                            let _e818: vec2<f32> = resLo;
                            let _e825: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e813.x, _e815.y) / _e818), vec2(0f), vec2(1f)));
                            s00_ = _e825;
                            let _e826: vec2<f32> = o2_;
                            let _e828: vec2<f32> = o1_;
                            let _e831: vec2<f32> = resLo;
                            let _e837: vec2<f32> = o2_;
                            let _e839: vec2<f32> = o1_;
                            let _e842: vec2<f32> = resLo;
                            let _e849: vec2<f32> = o2_;
                            let _e851: vec2<f32> = o1_;
                            let _e854: vec2<f32> = resLo;
                            let _e860: vec2<f32> = o2_;
                            let _e862: vec2<f32> = o1_;
                            let _e865: vec2<f32> = resLo;
                            let _e872: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e860.x, _e862.y) / _e865), vec2(0f), vec2(1f)));
                            s10_ = _e872;
                            let _e873: vec2<f32> = o1_;
                            let _e875: vec2<f32> = o2_;
                            let _e878: vec2<f32> = resLo;
                            let _e884: vec2<f32> = o1_;
                            let _e886: vec2<f32> = o2_;
                            let _e889: vec2<f32> = resLo;
                            let _e896: vec2<f32> = o1_;
                            let _e898: vec2<f32> = o2_;
                            let _e901: vec2<f32> = resLo;
                            let _e907: vec2<f32> = o1_;
                            let _e909: vec2<f32> = o2_;
                            let _e912: vec2<f32> = resLo;
                            let _e919: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e907.x, _e909.y) / _e912), vec2(0f), vec2(1f)));
                            s01_ = _e919;
                            let _e920: vec2<f32> = o2_;
                            let _e922: vec2<f32> = o2_;
                            let _e925: vec2<f32> = resLo;
                            let _e931: vec2<f32> = o2_;
                            let _e933: vec2<f32> = o2_;
                            let _e936: vec2<f32> = resLo;
                            let _e943: vec2<f32> = o2_;
                            let _e945: vec2<f32> = o2_;
                            let _e948: vec2<f32> = resLo;
                            let _e954: vec2<f32> = o2_;
                            let _e956: vec2<f32> = o2_;
                            let _e959: vec2<f32> = resLo;
                            let _e966: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e954.x, _e956.y) / _e959), vec2(0f), vec2(1f)));
                            s11_ = _e966;
                        }
                    } else {
                        let _e967: f32 = mLo;
                        if (_e967 < 4.5f) {
                            {
                                let _e970: vec2<f32> = o1_;
                                let _e972: vec2<f32> = o1_;
                                let _e975: vec2<f32> = resLo;
                                let _e981: vec2<f32> = o1_;
                                let _e983: vec2<f32> = o1_;
                                let _e986: vec2<f32> = resLo;
                                let _e993: vec2<f32> = o1_;
                                let _e995: vec2<f32> = o1_;
                                let _e998: vec2<f32> = resLo;
                                let _e1004: vec2<f32> = o1_;
                                let _e1006: vec2<f32> = o1_;
                                let _e1009: vec2<f32> = resLo;
                                let _e1016: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e1004.x, _e1006.y) / _e1009), vec2(0f), vec2(1f)));
                                s00_ = _e1016;
                                let _e1017: vec2<f32> = o2_;
                                let _e1019: vec2<f32> = o1_;
                                let _e1022: vec2<f32> = resLo;
                                let _e1028: vec2<f32> = o2_;
                                let _e1030: vec2<f32> = o1_;
                                let _e1033: vec2<f32> = resLo;
                                let _e1040: vec2<f32> = o2_;
                                let _e1042: vec2<f32> = o1_;
                                let _e1045: vec2<f32> = resLo;
                                let _e1051: vec2<f32> = o2_;
                                let _e1053: vec2<f32> = o1_;
                                let _e1056: vec2<f32> = resLo;
                                let _e1063: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e1051.x, _e1053.y) / _e1056), vec2(0f), vec2(1f)));
                                s10_ = _e1063;
                                let _e1064: vec2<f32> = o1_;
                                let _e1066: vec2<f32> = o2_;
                                let _e1069: vec2<f32> = resLo;
                                let _e1075: vec2<f32> = o1_;
                                let _e1077: vec2<f32> = o2_;
                                let _e1080: vec2<f32> = resLo;
                                let _e1087: vec2<f32> = o1_;
                                let _e1089: vec2<f32> = o2_;
                                let _e1092: vec2<f32> = resLo;
                                let _e1098: vec2<f32> = o1_;
                                let _e1100: vec2<f32> = o2_;
                                let _e1103: vec2<f32> = resLo;
                                let _e1110: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e1098.x, _e1100.y) / _e1103), vec2(0f), vec2(1f)));
                                s01_ = _e1110;
                                let _e1111: vec2<f32> = o2_;
                                let _e1113: vec2<f32> = o2_;
                                let _e1116: vec2<f32> = resLo;
                                let _e1122: vec2<f32> = o2_;
                                let _e1124: vec2<f32> = o2_;
                                let _e1127: vec2<f32> = resLo;
                                let _e1134: vec2<f32> = o2_;
                                let _e1136: vec2<f32> = o2_;
                                let _e1139: vec2<f32> = resLo;
                                let _e1145: vec2<f32> = o2_;
                                let _e1147: vec2<f32> = o2_;
                                let _e1150: vec2<f32> = resLo;
                                let _e1157: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e1145.x, _e1147.y) / _e1150), vec2(0f), vec2(1f)));
                                s11_ = _e1157;
                            }
                        } else {
                            let _e1158: f32 = mLo;
                            if (_e1158 < 5.5f) {
                                {
                                    let _e1161: vec2<f32> = o1_;
                                    let _e1163: vec2<f32> = o1_;
                                    let _e1166: vec2<f32> = resLo;
                                    let _e1172: vec2<f32> = o1_;
                                    let _e1174: vec2<f32> = o1_;
                                    let _e1177: vec2<f32> = resLo;
                                    let _e1184: vec2<f32> = o1_;
                                    let _e1186: vec2<f32> = o1_;
                                    let _e1189: vec2<f32> = resLo;
                                    let _e1195: vec2<f32> = o1_;
                                    let _e1197: vec2<f32> = o1_;
                                    let _e1200: vec2<f32> = resLo;
                                    let _e1207: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e1195.x, _e1197.y) / _e1200), vec2(0f), vec2(1f)));
                                    s00_ = _e1207;
                                    let _e1208: vec2<f32> = o2_;
                                    let _e1210: vec2<f32> = o1_;
                                    let _e1213: vec2<f32> = resLo;
                                    let _e1219: vec2<f32> = o2_;
                                    let _e1221: vec2<f32> = o1_;
                                    let _e1224: vec2<f32> = resLo;
                                    let _e1231: vec2<f32> = o2_;
                                    let _e1233: vec2<f32> = o1_;
                                    let _e1236: vec2<f32> = resLo;
                                    let _e1242: vec2<f32> = o2_;
                                    let _e1244: vec2<f32> = o1_;
                                    let _e1247: vec2<f32> = resLo;
                                    let _e1254: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e1242.x, _e1244.y) / _e1247), vec2(0f), vec2(1f)));
                                    s10_ = _e1254;
                                    let _e1255: vec2<f32> = o1_;
                                    let _e1257: vec2<f32> = o2_;
                                    let _e1260: vec2<f32> = resLo;
                                    let _e1266: vec2<f32> = o1_;
                                    let _e1268: vec2<f32> = o2_;
                                    let _e1271: vec2<f32> = resLo;
                                    let _e1278: vec2<f32> = o1_;
                                    let _e1280: vec2<f32> = o2_;
                                    let _e1283: vec2<f32> = resLo;
                                    let _e1289: vec2<f32> = o1_;
                                    let _e1291: vec2<f32> = o2_;
                                    let _e1294: vec2<f32> = resLo;
                                    let _e1301: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e1289.x, _e1291.y) / _e1294), vec2(0f), vec2(1f)));
                                    s01_ = _e1301;
                                    let _e1302: vec2<f32> = o2_;
                                    let _e1304: vec2<f32> = o2_;
                                    let _e1307: vec2<f32> = resLo;
                                    let _e1313: vec2<f32> = o2_;
                                    let _e1315: vec2<f32> = o2_;
                                    let _e1318: vec2<f32> = resLo;
                                    let _e1325: vec2<f32> = o2_;
                                    let _e1327: vec2<f32> = o2_;
                                    let _e1330: vec2<f32> = resLo;
                                    let _e1336: vec2<f32> = o2_;
                                    let _e1338: vec2<f32> = o2_;
                                    let _e1341: vec2<f32> = resLo;
                                    let _e1348: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e1336.x, _e1338.y) / _e1341), vec2(0f), vec2(1f)));
                                    s11_ = _e1348;
                                }
                            } else {
                                {
                                    let _e1349: vec2<f32> = o1_;
                                    let _e1351: vec2<f32> = o1_;
                                    let _e1354: vec2<f32> = resLo;
                                    let _e1360: vec2<f32> = o1_;
                                    let _e1362: vec2<f32> = o1_;
                                    let _e1365: vec2<f32> = resLo;
                                    let _e1372: vec2<f32> = o1_;
                                    let _e1374: vec2<f32> = o1_;
                                    let _e1377: vec2<f32> = resLo;
                                    let _e1383: vec2<f32> = o1_;
                                    let _e1385: vec2<f32> = o1_;
                                    let _e1388: vec2<f32> = resLo;
                                    let _e1395: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e1383.x, _e1385.y) / _e1388), vec2(0f), vec2(1f)));
                                    s00_ = _e1395;
                                    let _e1396: vec2<f32> = o2_;
                                    let _e1398: vec2<f32> = o1_;
                                    let _e1401: vec2<f32> = resLo;
                                    let _e1407: vec2<f32> = o2_;
                                    let _e1409: vec2<f32> = o1_;
                                    let _e1412: vec2<f32> = resLo;
                                    let _e1419: vec2<f32> = o2_;
                                    let _e1421: vec2<f32> = o1_;
                                    let _e1424: vec2<f32> = resLo;
                                    let _e1430: vec2<f32> = o2_;
                                    let _e1432: vec2<f32> = o1_;
                                    let _e1435: vec2<f32> = resLo;
                                    let _e1442: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e1430.x, _e1432.y) / _e1435), vec2(0f), vec2(1f)));
                                    s10_ = _e1442;
                                    let _e1443: vec2<f32> = o1_;
                                    let _e1445: vec2<f32> = o2_;
                                    let _e1448: vec2<f32> = resLo;
                                    let _e1454: vec2<f32> = o1_;
                                    let _e1456: vec2<f32> = o2_;
                                    let _e1459: vec2<f32> = resLo;
                                    let _e1466: vec2<f32> = o1_;
                                    let _e1468: vec2<f32> = o2_;
                                    let _e1471: vec2<f32> = resLo;
                                    let _e1477: vec2<f32> = o1_;
                                    let _e1479: vec2<f32> = o2_;
                                    let _e1482: vec2<f32> = resLo;
                                    let _e1489: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e1477.x, _e1479.y) / _e1482), vec2(0f), vec2(1f)));
                                    s01_ = _e1489;
                                    let _e1490: vec2<f32> = o2_;
                                    let _e1492: vec2<f32> = o2_;
                                    let _e1495: vec2<f32> = resLo;
                                    let _e1501: vec2<f32> = o2_;
                                    let _e1503: vec2<f32> = o2_;
                                    let _e1506: vec2<f32> = resLo;
                                    let _e1513: vec2<f32> = o2_;
                                    let _e1515: vec2<f32> = o2_;
                                    let _e1518: vec2<f32> = resLo;
                                    let _e1524: vec2<f32> = o2_;
                                    let _e1526: vec2<f32> = o2_;
                                    let _e1529: vec2<f32> = resLo;
                                    let _e1536: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e1524.x, _e1526.y) / _e1529), vec2(0f), vec2(1f)));
                                    s11_ = _e1536;
                                }
                            }
                        }
                    }
                }
            }
            let _e1537: vec2<f32> = w1_;
            let _e1539: vec2<f32> = w1_;
            let _e1542: vec4<f32> = s00_;
            let _e1544: vec2<f32> = w2_;
            let _e1546: vec2<f32> = w1_;
            let _e1549: vec4<f32> = s10_;
            let _e1552: vec2<f32> = w1_;
            let _e1554: vec2<f32> = w2_;
            let _e1557: vec4<f32> = s01_;
            let _e1560: vec2<f32> = w2_;
            let _e1562: vec2<f32> = w2_;
            let _e1565: vec4<f32> = s11_;
            clo = (((((_e1537.x * _e1539.y) * _e1542) + ((_e1544.x * _e1546.y) * _e1549)) + ((_e1552.x * _e1554.y) * _e1557)) + ((_e1560.x * _e1562.y) * _e1565));
        }
    }
    let _e1568: f32 = mLo;
    mHi = (_e1568 + 1f);
    let _e1576: f32 = mHi;
    scale = (1f / pow(2f, _e1576));
    let _e1579: vec2<f32> = coord;
    let _e1580: f32 = scale;
    dHi = ((_e1579 * _e1580) - vec2(0.5f));
    let _e1587: vec2<f32> = dHi;
    cHi_1 = floor(_e1587);
    let _e1590: vec2<f32> = cHi_1;
    let _e1591: vec2<f32> = dHi;
    xHi = ((_e1590 - _e1591) + vec2(1f));
    let _e1597: vec2<f32> = dHi;
    let _e1598: vec2<f32> = cHi_1;
    XHi = (_e1597 - _e1598);
    let _e1601: vec2<f32> = xHi;
    let _e1602: vec2<f32> = xHi;
    let _e1604: vec2<f32> = xHi;
    x3Hi = ((_e1601 * _e1602) * _e1604);
    let _e1608: vec2<f32> = xHi;
    let _e1610: vec2<f32> = xHi;
    let _e1613: vec2<f32> = xHi;
    coeffHi = ((((0.5f * _e1608) * _e1610) + (0.5f * _e1613)) + vec2(0.166667f));
    let _e1622: vec2<f32> = x3Hi;
    let _e1624: vec2<f32> = coeffHi;
    w1Hi = ((-0.333333f * _e1622) + _e1624);
    let _e1628: vec2<f32> = w1Hi;
    w2Hi = (vec2(1f) - _e1628);
    let _e1634: vec2<f32> = x3Hi;
    let _e1636: vec2<f32> = coeffHi;
    let _e1638: vec2<f32> = w1Hi;
    let _e1640: vec2<f32> = cHi_1;
    o1Hi = (((((-0.5f * _e1634) + _e1636) / _e1638) + _e1640) - vec2(0.5f));
    let _e1646: vec2<f32> = XHi;
    let _e1647: vec2<f32> = XHi;
    let _e1649: vec2<f32> = XHi;
    let _e1654: vec2<f32> = w2Hi;
    let _e1656: vec2<f32> = cHi_1;
    o2Hi = ((((((_e1646 * _e1647) * _e1649) / vec2(6f)) / _e1654) + _e1656) + vec2(1.5f));
    let _e1662: vec2<f32> = uEnlargedTextureSize;
    let _e1666: f32 = mHi;
    resHi = (_e1662 / vec2(pow(2f, _e1666)));
    let _e1675: f32 = mHi;
    if (_e1675 < 1.5f) {
        {
            let _e1678: vec2<f32> = o1Hi;
            let _e1680: vec2<f32> = o1Hi;
            let _e1683: vec2<f32> = resHi;
            let _e1689: vec2<f32> = o1Hi;
            let _e1691: vec2<f32> = o1Hi;
            let _e1694: vec2<f32> = resHi;
            let _e1701: vec2<f32> = o1Hi;
            let _e1703: vec2<f32> = o1Hi;
            let _e1706: vec2<f32> = resHi;
            let _e1712: vec2<f32> = o1Hi;
            let _e1714: vec2<f32> = o1Hi;
            let _e1717: vec2<f32> = resHi;
            let _e1724: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e1712.x, _e1714.y) / _e1717), vec2(0f), vec2(1f)));
            h00_ = _e1724;
            let _e1725: vec2<f32> = o2Hi;
            let _e1727: vec2<f32> = o1Hi;
            let _e1730: vec2<f32> = resHi;
            let _e1736: vec2<f32> = o2Hi;
            let _e1738: vec2<f32> = o1Hi;
            let _e1741: vec2<f32> = resHi;
            let _e1748: vec2<f32> = o2Hi;
            let _e1750: vec2<f32> = o1Hi;
            let _e1753: vec2<f32> = resHi;
            let _e1759: vec2<f32> = o2Hi;
            let _e1761: vec2<f32> = o1Hi;
            let _e1764: vec2<f32> = resHi;
            let _e1771: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e1759.x, _e1761.y) / _e1764), vec2(0f), vec2(1f)));
            h10_ = _e1771;
            let _e1772: vec2<f32> = o1Hi;
            let _e1774: vec2<f32> = o2Hi;
            let _e1777: vec2<f32> = resHi;
            let _e1783: vec2<f32> = o1Hi;
            let _e1785: vec2<f32> = o2Hi;
            let _e1788: vec2<f32> = resHi;
            let _e1795: vec2<f32> = o1Hi;
            let _e1797: vec2<f32> = o2Hi;
            let _e1800: vec2<f32> = resHi;
            let _e1806: vec2<f32> = o1Hi;
            let _e1808: vec2<f32> = o2Hi;
            let _e1811: vec2<f32> = resHi;
            let _e1818: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e1806.x, _e1808.y) / _e1811), vec2(0f), vec2(1f)));
            h01_ = _e1818;
            let _e1819: vec2<f32> = o2Hi;
            let _e1821: vec2<f32> = o2Hi;
            let _e1824: vec2<f32> = resHi;
            let _e1830: vec2<f32> = o2Hi;
            let _e1832: vec2<f32> = o2Hi;
            let _e1835: vec2<f32> = resHi;
            let _e1842: vec2<f32> = o2Hi;
            let _e1844: vec2<f32> = o2Hi;
            let _e1847: vec2<f32> = resHi;
            let _e1853: vec2<f32> = o2Hi;
            let _e1855: vec2<f32> = o2Hi;
            let _e1858: vec2<f32> = resHi;
            let _e1865: vec4<f32> = sample_pass_Downsample_mip1_(clamp((vec2<f32>(_e1853.x, _e1855.y) / _e1858), vec2(0f), vec2(1f)));
            h11_ = _e1865;
        }
    } else {
        let _e1866: f32 = mHi;
        if (_e1866 < 2.5f) {
            {
                let _e1869: vec2<f32> = o1Hi;
                let _e1871: vec2<f32> = o1Hi;
                let _e1874: vec2<f32> = resHi;
                let _e1880: vec2<f32> = o1Hi;
                let _e1882: vec2<f32> = o1Hi;
                let _e1885: vec2<f32> = resHi;
                let _e1892: vec2<f32> = o1Hi;
                let _e1894: vec2<f32> = o1Hi;
                let _e1897: vec2<f32> = resHi;
                let _e1903: vec2<f32> = o1Hi;
                let _e1905: vec2<f32> = o1Hi;
                let _e1908: vec2<f32> = resHi;
                let _e1915: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e1903.x, _e1905.y) / _e1908), vec2(0f), vec2(1f)));
                h00_ = _e1915;
                let _e1916: vec2<f32> = o2Hi;
                let _e1918: vec2<f32> = o1Hi;
                let _e1921: vec2<f32> = resHi;
                let _e1927: vec2<f32> = o2Hi;
                let _e1929: vec2<f32> = o1Hi;
                let _e1932: vec2<f32> = resHi;
                let _e1939: vec2<f32> = o2Hi;
                let _e1941: vec2<f32> = o1Hi;
                let _e1944: vec2<f32> = resHi;
                let _e1950: vec2<f32> = o2Hi;
                let _e1952: vec2<f32> = o1Hi;
                let _e1955: vec2<f32> = resHi;
                let _e1962: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e1950.x, _e1952.y) / _e1955), vec2(0f), vec2(1f)));
                h10_ = _e1962;
                let _e1963: vec2<f32> = o1Hi;
                let _e1965: vec2<f32> = o2Hi;
                let _e1968: vec2<f32> = resHi;
                let _e1974: vec2<f32> = o1Hi;
                let _e1976: vec2<f32> = o2Hi;
                let _e1979: vec2<f32> = resHi;
                let _e1986: vec2<f32> = o1Hi;
                let _e1988: vec2<f32> = o2Hi;
                let _e1991: vec2<f32> = resHi;
                let _e1997: vec2<f32> = o1Hi;
                let _e1999: vec2<f32> = o2Hi;
                let _e2002: vec2<f32> = resHi;
                let _e2009: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e1997.x, _e1999.y) / _e2002), vec2(0f), vec2(1f)));
                h01_ = _e2009;
                let _e2010: vec2<f32> = o2Hi;
                let _e2012: vec2<f32> = o2Hi;
                let _e2015: vec2<f32> = resHi;
                let _e2021: vec2<f32> = o2Hi;
                let _e2023: vec2<f32> = o2Hi;
                let _e2026: vec2<f32> = resHi;
                let _e2033: vec2<f32> = o2Hi;
                let _e2035: vec2<f32> = o2Hi;
                let _e2038: vec2<f32> = resHi;
                let _e2044: vec2<f32> = o2Hi;
                let _e2046: vec2<f32> = o2Hi;
                let _e2049: vec2<f32> = resHi;
                let _e2056: vec4<f32> = sample_pass_Downsample_mip2_(clamp((vec2<f32>(_e2044.x, _e2046.y) / _e2049), vec2(0f), vec2(1f)));
                h11_ = _e2056;
            }
        } else {
            let _e2057: f32 = mHi;
            if (_e2057 < 3.5f) {
                {
                    let _e2060: vec2<f32> = o1Hi;
                    let _e2062: vec2<f32> = o1Hi;
                    let _e2065: vec2<f32> = resHi;
                    let _e2071: vec2<f32> = o1Hi;
                    let _e2073: vec2<f32> = o1Hi;
                    let _e2076: vec2<f32> = resHi;
                    let _e2083: vec2<f32> = o1Hi;
                    let _e2085: vec2<f32> = o1Hi;
                    let _e2088: vec2<f32> = resHi;
                    let _e2094: vec2<f32> = o1Hi;
                    let _e2096: vec2<f32> = o1Hi;
                    let _e2099: vec2<f32> = resHi;
                    let _e2106: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e2094.x, _e2096.y) / _e2099), vec2(0f), vec2(1f)));
                    h00_ = _e2106;
                    let _e2107: vec2<f32> = o2Hi;
                    let _e2109: vec2<f32> = o1Hi;
                    let _e2112: vec2<f32> = resHi;
                    let _e2118: vec2<f32> = o2Hi;
                    let _e2120: vec2<f32> = o1Hi;
                    let _e2123: vec2<f32> = resHi;
                    let _e2130: vec2<f32> = o2Hi;
                    let _e2132: vec2<f32> = o1Hi;
                    let _e2135: vec2<f32> = resHi;
                    let _e2141: vec2<f32> = o2Hi;
                    let _e2143: vec2<f32> = o1Hi;
                    let _e2146: vec2<f32> = resHi;
                    let _e2153: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e2141.x, _e2143.y) / _e2146), vec2(0f), vec2(1f)));
                    h10_ = _e2153;
                    let _e2154: vec2<f32> = o1Hi;
                    let _e2156: vec2<f32> = o2Hi;
                    let _e2159: vec2<f32> = resHi;
                    let _e2165: vec2<f32> = o1Hi;
                    let _e2167: vec2<f32> = o2Hi;
                    let _e2170: vec2<f32> = resHi;
                    let _e2177: vec2<f32> = o1Hi;
                    let _e2179: vec2<f32> = o2Hi;
                    let _e2182: vec2<f32> = resHi;
                    let _e2188: vec2<f32> = o1Hi;
                    let _e2190: vec2<f32> = o2Hi;
                    let _e2193: vec2<f32> = resHi;
                    let _e2200: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e2188.x, _e2190.y) / _e2193), vec2(0f), vec2(1f)));
                    h01_ = _e2200;
                    let _e2201: vec2<f32> = o2Hi;
                    let _e2203: vec2<f32> = o2Hi;
                    let _e2206: vec2<f32> = resHi;
                    let _e2212: vec2<f32> = o2Hi;
                    let _e2214: vec2<f32> = o2Hi;
                    let _e2217: vec2<f32> = resHi;
                    let _e2224: vec2<f32> = o2Hi;
                    let _e2226: vec2<f32> = o2Hi;
                    let _e2229: vec2<f32> = resHi;
                    let _e2235: vec2<f32> = o2Hi;
                    let _e2237: vec2<f32> = o2Hi;
                    let _e2240: vec2<f32> = resHi;
                    let _e2247: vec4<f32> = sample_pass_Downsample_mip3_(clamp((vec2<f32>(_e2235.x, _e2237.y) / _e2240), vec2(0f), vec2(1f)));
                    h11_ = _e2247;
                }
            } else {
                let _e2248: f32 = mHi;
                if (_e2248 < 4.5f) {
                    {
                        let _e2251: vec2<f32> = o1Hi;
                        let _e2253: vec2<f32> = o1Hi;
                        let _e2256: vec2<f32> = resHi;
                        let _e2262: vec2<f32> = o1Hi;
                        let _e2264: vec2<f32> = o1Hi;
                        let _e2267: vec2<f32> = resHi;
                        let _e2274: vec2<f32> = o1Hi;
                        let _e2276: vec2<f32> = o1Hi;
                        let _e2279: vec2<f32> = resHi;
                        let _e2285: vec2<f32> = o1Hi;
                        let _e2287: vec2<f32> = o1Hi;
                        let _e2290: vec2<f32> = resHi;
                        let _e2297: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e2285.x, _e2287.y) / _e2290), vec2(0f), vec2(1f)));
                        h00_ = _e2297;
                        let _e2298: vec2<f32> = o2Hi;
                        let _e2300: vec2<f32> = o1Hi;
                        let _e2303: vec2<f32> = resHi;
                        let _e2309: vec2<f32> = o2Hi;
                        let _e2311: vec2<f32> = o1Hi;
                        let _e2314: vec2<f32> = resHi;
                        let _e2321: vec2<f32> = o2Hi;
                        let _e2323: vec2<f32> = o1Hi;
                        let _e2326: vec2<f32> = resHi;
                        let _e2332: vec2<f32> = o2Hi;
                        let _e2334: vec2<f32> = o1Hi;
                        let _e2337: vec2<f32> = resHi;
                        let _e2344: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e2332.x, _e2334.y) / _e2337), vec2(0f), vec2(1f)));
                        h10_ = _e2344;
                        let _e2345: vec2<f32> = o1Hi;
                        let _e2347: vec2<f32> = o2Hi;
                        let _e2350: vec2<f32> = resHi;
                        let _e2356: vec2<f32> = o1Hi;
                        let _e2358: vec2<f32> = o2Hi;
                        let _e2361: vec2<f32> = resHi;
                        let _e2368: vec2<f32> = o1Hi;
                        let _e2370: vec2<f32> = o2Hi;
                        let _e2373: vec2<f32> = resHi;
                        let _e2379: vec2<f32> = o1Hi;
                        let _e2381: vec2<f32> = o2Hi;
                        let _e2384: vec2<f32> = resHi;
                        let _e2391: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e2379.x, _e2381.y) / _e2384), vec2(0f), vec2(1f)));
                        h01_ = _e2391;
                        let _e2392: vec2<f32> = o2Hi;
                        let _e2394: vec2<f32> = o2Hi;
                        let _e2397: vec2<f32> = resHi;
                        let _e2403: vec2<f32> = o2Hi;
                        let _e2405: vec2<f32> = o2Hi;
                        let _e2408: vec2<f32> = resHi;
                        let _e2415: vec2<f32> = o2Hi;
                        let _e2417: vec2<f32> = o2Hi;
                        let _e2420: vec2<f32> = resHi;
                        let _e2426: vec2<f32> = o2Hi;
                        let _e2428: vec2<f32> = o2Hi;
                        let _e2431: vec2<f32> = resHi;
                        let _e2438: vec4<f32> = sample_pass_Downsample_mip4_(clamp((vec2<f32>(_e2426.x, _e2428.y) / _e2431), vec2(0f), vec2(1f)));
                        h11_ = _e2438;
                    }
                } else {
                    let _e2439: f32 = mHi;
                    if (_e2439 < 5.5f) {
                        {
                            let _e2442: vec2<f32> = o1Hi;
                            let _e2444: vec2<f32> = o1Hi;
                            let _e2447: vec2<f32> = resHi;
                            let _e2453: vec2<f32> = o1Hi;
                            let _e2455: vec2<f32> = o1Hi;
                            let _e2458: vec2<f32> = resHi;
                            let _e2465: vec2<f32> = o1Hi;
                            let _e2467: vec2<f32> = o1Hi;
                            let _e2470: vec2<f32> = resHi;
                            let _e2476: vec2<f32> = o1Hi;
                            let _e2478: vec2<f32> = o1Hi;
                            let _e2481: vec2<f32> = resHi;
                            let _e2488: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e2476.x, _e2478.y) / _e2481), vec2(0f), vec2(1f)));
                            h00_ = _e2488;
                            let _e2489: vec2<f32> = o2Hi;
                            let _e2491: vec2<f32> = o1Hi;
                            let _e2494: vec2<f32> = resHi;
                            let _e2500: vec2<f32> = o2Hi;
                            let _e2502: vec2<f32> = o1Hi;
                            let _e2505: vec2<f32> = resHi;
                            let _e2512: vec2<f32> = o2Hi;
                            let _e2514: vec2<f32> = o1Hi;
                            let _e2517: vec2<f32> = resHi;
                            let _e2523: vec2<f32> = o2Hi;
                            let _e2525: vec2<f32> = o1Hi;
                            let _e2528: vec2<f32> = resHi;
                            let _e2535: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e2523.x, _e2525.y) / _e2528), vec2(0f), vec2(1f)));
                            h10_ = _e2535;
                            let _e2536: vec2<f32> = o1Hi;
                            let _e2538: vec2<f32> = o2Hi;
                            let _e2541: vec2<f32> = resHi;
                            let _e2547: vec2<f32> = o1Hi;
                            let _e2549: vec2<f32> = o2Hi;
                            let _e2552: vec2<f32> = resHi;
                            let _e2559: vec2<f32> = o1Hi;
                            let _e2561: vec2<f32> = o2Hi;
                            let _e2564: vec2<f32> = resHi;
                            let _e2570: vec2<f32> = o1Hi;
                            let _e2572: vec2<f32> = o2Hi;
                            let _e2575: vec2<f32> = resHi;
                            let _e2582: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e2570.x, _e2572.y) / _e2575), vec2(0f), vec2(1f)));
                            h01_ = _e2582;
                            let _e2583: vec2<f32> = o2Hi;
                            let _e2585: vec2<f32> = o2Hi;
                            let _e2588: vec2<f32> = resHi;
                            let _e2594: vec2<f32> = o2Hi;
                            let _e2596: vec2<f32> = o2Hi;
                            let _e2599: vec2<f32> = resHi;
                            let _e2606: vec2<f32> = o2Hi;
                            let _e2608: vec2<f32> = o2Hi;
                            let _e2611: vec2<f32> = resHi;
                            let _e2617: vec2<f32> = o2Hi;
                            let _e2619: vec2<f32> = o2Hi;
                            let _e2622: vec2<f32> = resHi;
                            let _e2629: vec4<f32> = sample_pass_Downsample_mip5_(clamp((vec2<f32>(_e2617.x, _e2619.y) / _e2622), vec2(0f), vec2(1f)));
                            h11_ = _e2629;
                        }
                    } else {
                        let _e2630: f32 = mHi;
                        if (_e2630 < 6.5f) {
                            {
                                let _e2633: vec2<f32> = o1Hi;
                                let _e2635: vec2<f32> = o1Hi;
                                let _e2638: vec2<f32> = resHi;
                                let _e2644: vec2<f32> = o1Hi;
                                let _e2646: vec2<f32> = o1Hi;
                                let _e2649: vec2<f32> = resHi;
                                let _e2656: vec2<f32> = o1Hi;
                                let _e2658: vec2<f32> = o1Hi;
                                let _e2661: vec2<f32> = resHi;
                                let _e2667: vec2<f32> = o1Hi;
                                let _e2669: vec2<f32> = o1Hi;
                                let _e2672: vec2<f32> = resHi;
                                let _e2679: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2667.x, _e2669.y) / _e2672), vec2(0f), vec2(1f)));
                                h00_ = _e2679;
                                let _e2680: vec2<f32> = o2Hi;
                                let _e2682: vec2<f32> = o1Hi;
                                let _e2685: vec2<f32> = resHi;
                                let _e2691: vec2<f32> = o2Hi;
                                let _e2693: vec2<f32> = o1Hi;
                                let _e2696: vec2<f32> = resHi;
                                let _e2703: vec2<f32> = o2Hi;
                                let _e2705: vec2<f32> = o1Hi;
                                let _e2708: vec2<f32> = resHi;
                                let _e2714: vec2<f32> = o2Hi;
                                let _e2716: vec2<f32> = o1Hi;
                                let _e2719: vec2<f32> = resHi;
                                let _e2726: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2714.x, _e2716.y) / _e2719), vec2(0f), vec2(1f)));
                                h10_ = _e2726;
                                let _e2727: vec2<f32> = o1Hi;
                                let _e2729: vec2<f32> = o2Hi;
                                let _e2732: vec2<f32> = resHi;
                                let _e2738: vec2<f32> = o1Hi;
                                let _e2740: vec2<f32> = o2Hi;
                                let _e2743: vec2<f32> = resHi;
                                let _e2750: vec2<f32> = o1Hi;
                                let _e2752: vec2<f32> = o2Hi;
                                let _e2755: vec2<f32> = resHi;
                                let _e2761: vec2<f32> = o1Hi;
                                let _e2763: vec2<f32> = o2Hi;
                                let _e2766: vec2<f32> = resHi;
                                let _e2773: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2761.x, _e2763.y) / _e2766), vec2(0f), vec2(1f)));
                                h01_ = _e2773;
                                let _e2774: vec2<f32> = o2Hi;
                                let _e2776: vec2<f32> = o2Hi;
                                let _e2779: vec2<f32> = resHi;
                                let _e2785: vec2<f32> = o2Hi;
                                let _e2787: vec2<f32> = o2Hi;
                                let _e2790: vec2<f32> = resHi;
                                let _e2797: vec2<f32> = o2Hi;
                                let _e2799: vec2<f32> = o2Hi;
                                let _e2802: vec2<f32> = resHi;
                                let _e2808: vec2<f32> = o2Hi;
                                let _e2810: vec2<f32> = o2Hi;
                                let _e2813: vec2<f32> = resHi;
                                let _e2820: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2808.x, _e2810.y) / _e2813), vec2(0f), vec2(1f)));
                                h11_ = _e2820;
                            }
                        } else {
                            {
                                let _e2821: vec2<f32> = o1Hi;
                                let _e2823: vec2<f32> = o1Hi;
                                let _e2826: vec2<f32> = resHi;
                                let _e2832: vec2<f32> = o1Hi;
                                let _e2834: vec2<f32> = o1Hi;
                                let _e2837: vec2<f32> = resHi;
                                let _e2844: vec2<f32> = o1Hi;
                                let _e2846: vec2<f32> = o1Hi;
                                let _e2849: vec2<f32> = resHi;
                                let _e2855: vec2<f32> = o1Hi;
                                let _e2857: vec2<f32> = o1Hi;
                                let _e2860: vec2<f32> = resHi;
                                let _e2867: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2855.x, _e2857.y) / _e2860), vec2(0f), vec2(1f)));
                                h00_ = _e2867;
                                let _e2868: vec2<f32> = o2Hi;
                                let _e2870: vec2<f32> = o1Hi;
                                let _e2873: vec2<f32> = resHi;
                                let _e2879: vec2<f32> = o2Hi;
                                let _e2881: vec2<f32> = o1Hi;
                                let _e2884: vec2<f32> = resHi;
                                let _e2891: vec2<f32> = o2Hi;
                                let _e2893: vec2<f32> = o1Hi;
                                let _e2896: vec2<f32> = resHi;
                                let _e2902: vec2<f32> = o2Hi;
                                let _e2904: vec2<f32> = o1Hi;
                                let _e2907: vec2<f32> = resHi;
                                let _e2914: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2902.x, _e2904.y) / _e2907), vec2(0f), vec2(1f)));
                                h10_ = _e2914;
                                let _e2915: vec2<f32> = o1Hi;
                                let _e2917: vec2<f32> = o2Hi;
                                let _e2920: vec2<f32> = resHi;
                                let _e2926: vec2<f32> = o1Hi;
                                let _e2928: vec2<f32> = o2Hi;
                                let _e2931: vec2<f32> = resHi;
                                let _e2938: vec2<f32> = o1Hi;
                                let _e2940: vec2<f32> = o2Hi;
                                let _e2943: vec2<f32> = resHi;
                                let _e2949: vec2<f32> = o1Hi;
                                let _e2951: vec2<f32> = o2Hi;
                                let _e2954: vec2<f32> = resHi;
                                let _e2961: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2949.x, _e2951.y) / _e2954), vec2(0f), vec2(1f)));
                                h01_ = _e2961;
                                let _e2962: vec2<f32> = o2Hi;
                                let _e2964: vec2<f32> = o2Hi;
                                let _e2967: vec2<f32> = resHi;
                                let _e2973: vec2<f32> = o2Hi;
                                let _e2975: vec2<f32> = o2Hi;
                                let _e2978: vec2<f32> = resHi;
                                let _e2985: vec2<f32> = o2Hi;
                                let _e2987: vec2<f32> = o2Hi;
                                let _e2990: vec2<f32> = resHi;
                                let _e2996: vec2<f32> = o2Hi;
                                let _e2998: vec2<f32> = o2Hi;
                                let _e3001: vec2<f32> = resHi;
                                let _e3008: vec4<f32> = sample_pass_Downsample_mip6_(clamp((vec2<f32>(_e2996.x, _e2998.y) / _e3001), vec2(0f), vec2(1f)));
                                h11_ = _e3008;
                            }
                        }
                    }
                }
            }
        }
    }
    let _e3009: vec2<f32> = w1Hi;
    let _e3011: vec2<f32> = w1Hi;
    let _e3014: vec4<f32> = h00_;
    let _e3016: vec2<f32> = w2Hi;
    let _e3018: vec2<f32> = w1Hi;
    let _e3021: vec4<f32> = h10_;
    let _e3024: vec2<f32> = w1Hi;
    let _e3026: vec2<f32> = w2Hi;
    let _e3029: vec4<f32> = h01_;
    let _e3032: vec2<f32> = w2Hi;
    let _e3034: vec2<f32> = w2Hi;
    let _e3037: vec4<f32> = h11_;
    cHi = (((((_e3009.x * _e3011.y) * _e3014) + ((_e3016.x * _e3018.y) * _e3021)) + ((_e3024.x * _e3026.y) * _e3029)) + ((_e3032.x * _e3034.y) * _e3037));
    let _e3042: f32 = m_1;
    let _e3043: f32 = mLo;
    let _e3045: vec4<f32> = clo;
    let _e3046: vec4<f32> = cHi;
    let _e3047: f32 = m_1;
    let _e3048: f32 = mLo;
    output = mix(_e3045, _e3046, vec4((_e3047 - _e3048)));
    let _e3052: vec4<f32> = output;
    return _e3052;
}

fn sample_pass_Downsample_mip1_(uv_in: vec2f) -> vec4f {
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip1, pass_samp_Downsample_mip1, uv_flipped);
}

fn sample_pass_Downsample_mip2_(uv_in: vec2f) -> vec4f {
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip2, pass_samp_Downsample_mip2, uv_flipped);
}

fn sample_pass_Downsample_mip3_(uv_in: vec2f) -> vec4f {
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip3, pass_samp_Downsample_mip3, uv_flipped);
}

fn sample_pass_Downsample_mip4_(uv_in: vec2f) -> vec4f {
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip4, pass_samp_Downsample_mip4, uv_flipped);
}

fn sample_pass_Downsample_mip5_(uv_in: vec2f) -> vec4f {
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip5, pass_samp_Downsample_mip5, uv_flipped);
}

fn sample_pass_Downsample_mip6_(uv_in: vec2f) -> vec4f {
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_Downsample_mip6, pass_samp_Downsample_mip6, uv_flipped);
}

fn sample_pass_RenderPass_85_(uv_in: vec2f) -> vec4f {
    let uv_flipped = vec2f(uv_in.x, 1.0 - uv_in.y);
    return textureSample(pass_tex_RenderPass_85, pass_samp_RenderPass_85, uv_flipped);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_calculateMaskLinear_out: f32;
    {
        let xy = in.local_px;
        let start_x = (graph_inputs.node_FloatInput_95_0ec7eb20).x;
        var output: f32;
        output = mc_MathClosure_calculateMaskLinear(in.uv, xy, start_x);
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
