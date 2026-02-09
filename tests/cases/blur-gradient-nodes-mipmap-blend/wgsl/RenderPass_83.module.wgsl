
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
var pass_tex_RenderPass_7: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_RenderPass_7: sampler;

@group(1) @binding(2)
var pass_tex_Downsample_10: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_Downsample_10: sampler;

@group(1) @binding(4)
var pass_tex_Downsample_12: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp_Downsample_12: sampler;

@group(1) @binding(6)
var pass_tex_Downsample_13: texture_2d<f32>;

@group(1) @binding(7)
var pass_samp_Downsample_13: sampler;

@group(1) @binding(8)
var pass_tex_Downsample_16: texture_2d<f32>;

@group(1) @binding(9)
var pass_samp_Downsample_16: sampler;

@group(1) @binding(10)
var pass_tex_Downsample_18: texture_2d<f32>;

@group(1) @binding(11)
var pass_samp_Downsample_18: sampler;

@group(1) @binding(12)
var pass_tex_Downsample_20: texture_2d<f32>;

@group(1) @binding(13)
var pass_samp_Downsample_20: sampler;

@group(1) @binding(14)
var pass_tex_Downsample_22: texture_2d<f32>;

@group(1) @binding(15)
var pass_samp_Downsample_22: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_GroupInstance_81_MathClosure_30_(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
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

fn mc_MathClosure_82_(uv: vec2<f32>, uv_1: vec2<f32>) -> f32 {
    var uv_2: vec2<f32>;
    var uv_3: vec2<f32>;
    var output: f32 = 0f;
    var y: f32;
    var mask: f32;
    var radius: f32;
    var m: f32;

    uv_2 = uv;
    uv_3 = uv_1;
    let _e7: vec2<f32> = uv_3;
    let _e11: vec2<f32> = uv_3;
    y = clamp(_e11.y, 0f, 1f);
    let _e22: f32 = y;
    mask = smoothstep(1f, 0f, _e22);
    let _e27: f32 = mask;
    radius = (0f + (50f * _e27));
    let _e31: f32 = radius;
    let _e35: f32 = radius;
    let _e40: f32 = radius;
    let _e44: f32 = radius;
    m = log2(max((_e44 * 1.333333f), 0.000001f));
    let _e54: f32 = m;
    output = clamp(_e54, 0f, 6f);
    let _e58: f32 = output;
    return _e58;
}

fn mc_MathClosure_86_(uv: vec2<f32>, m: f32) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var m_1: f32;
    var output: vec3<f32> = vec3(0f);
    var mLo: f32;
    var mHi: f32;
    var factor: f32;

    uv_1 = uv;
    m_1 = m;
    let _e8: f32 = m_1;
    mLo = floor(_e8);
    let _e11: f32 = mLo;
    let _e15: f32 = mLo;
    mHi = min((_e15 + 1f), 6f);
    let _e21: f32 = m_1;
    let _e22: f32 = mLo;
    factor = (_e21 - _e22);
    let _e25: f32 = mLo;
    let _e26: f32 = mHi;
    let _e27: f32 = factor;
    output = vec3<f32>(_e25, _e26, _e27);
    let _e29: vec3<f32> = output;
    return _e29;
}

fn mc_MathClosure_89_(uv: vec2<f32>, levels: vec3<f32>, m0_: vec4<f32>, m1_: vec4<f32>, m2_: vec4<f32>, m3_: vec4<f32>, m4_: vec4<f32>, m5_: vec4<f32>, m6_: vec4<f32>, m7_: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var levels_1: vec3<f32>;
    var m0_1: vec4<f32>;
    var m1_1: vec4<f32>;
    var m2_1: vec4<f32>;
    var m3_1: vec4<f32>;
    var m4_1: vec4<f32>;
    var m5_1: vec4<f32>;
    var m6_1: vec4<f32>;
    var m7_1: vec4<f32>;
    var output: vec4<f32> = vec4(0f);
    var mLo: f32;
    var mHi: f32;
    var factor: f32;
    var clo: vec4<f32>;
    var cHi: vec4<f32>;

    uv_1 = uv;
    levels_1 = levels;
    m0_1 = m0_;
    m1_1 = m1_;
    m2_1 = m2_;
    m3_1 = m3_;
    m4_1 = m4_;
    m5_1 = m5_;
    m6_1 = m6_;
    m7_1 = m7_;
    let _e23: vec3<f32> = levels_1;
    mLo = _e23.x;
    let _e26: vec3<f32> = levels_1;
    mHi = _e26.y;
    let _e29: vec3<f32> = levels_1;
    let _e33: vec3<f32> = levels_1;
    factor = clamp(_e33.z, 0f, 1f);
    let _e40: f32 = mLo;
    if (_e40 < 0.5f) {
        {
            let _e43: vec4<f32> = m0_1;
            clo = _e43;
        }
    } else {
        let _e44: f32 = mLo;
        if (_e44 < 1.5f) {
            {
                let _e47: vec4<f32> = m1_1;
                clo = _e47;
            }
        } else {
            let _e48: f32 = mLo;
            if (_e48 < 2.5f) {
                {
                    let _e51: vec4<f32> = m2_1;
                    clo = _e51;
                }
            } else {
                let _e52: f32 = mLo;
                if (_e52 < 3.5f) {
                    {
                        let _e55: vec4<f32> = m3_1;
                        clo = _e55;
                    }
                } else {
                    let _e56: f32 = mLo;
                    if (_e56 < 4.5f) {
                        {
                            let _e59: vec4<f32> = m4_1;
                            clo = _e59;
                        }
                    } else {
                        let _e60: f32 = mLo;
                        if (_e60 < 5.5f) {
                            {
                                let _e63: vec4<f32> = m5_1;
                                clo = _e63;
                            }
                        } else {
                            let _e64: f32 = mLo;
                            if (_e64 < 6.5f) {
                                {
                                    let _e67: vec4<f32> = m6_1;
                                    clo = _e67;
                                }
                            } else {
                                {
                                    let _e68: vec4<f32> = m7_1;
                                    clo = _e68;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e70: f32 = mHi;
    if (_e70 < 0.5f) {
        {
            let _e73: vec4<f32> = m0_1;
            cHi = _e73;
        }
    } else {
        let _e74: f32 = mHi;
        if (_e74 < 1.5f) {
            {
                let _e77: vec4<f32> = m1_1;
                cHi = _e77;
            }
        } else {
            let _e78: f32 = mHi;
            if (_e78 < 2.5f) {
                {
                    let _e81: vec4<f32> = m2_1;
                    cHi = _e81;
                }
            } else {
                let _e82: f32 = mHi;
                if (_e82 < 3.5f) {
                    {
                        let _e85: vec4<f32> = m3_1;
                        cHi = _e85;
                    }
                } else {
                    let _e86: f32 = mHi;
                    if (_e86 < 4.5f) {
                        {
                            let _e89: vec4<f32> = m4_1;
                            cHi = _e89;
                        }
                    } else {
                        let _e90: f32 = mHi;
                        if (_e90 < 5.5f) {
                            {
                                let _e93: vec4<f32> = m5_1;
                                cHi = _e93;
                            }
                        } else {
                            let _e94: f32 = mHi;
                            if (_e94 < 6.5f) {
                                {
                                    let _e97: vec4<f32> = m6_1;
                                    cHi = _e97;
                                }
                            } else {
                                {
                                    let _e98: vec4<f32> = m7_1;
                                    cHi = _e98;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e102: vec4<f32> = clo;
    let _e103: vec4<f32> = cHi;
    let _e104: f32 = factor;
    output = mix(_e102, _e103, vec4(_e104));
    let _e107: vec4<f32> = output;
    return _e107;
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
        var mc_GroupInstance_81_MathClosure_30_out: vec2f;
    {
        let xy = in.local_px;
        let size = in.geo_size_px;
        var output: vec2f;
        output = mc_GroupInstance_81_MathClosure_30_(in.uv, xy, size);
        mc_GroupInstance_81_MathClosure_30_out = output;
    }
    var mc_MathClosure_82_out: f32;
    {
        let uv = mc_GroupInstance_81_MathClosure_30_out;
        var output: f32;
        output = mc_MathClosure_82_(in.uv, uv);
        mc_MathClosure_82_out = output;
    }
    var mc_MathClosure_86_out: vec3f;
    {
        let m = mc_MathClosure_82_out;
        var output: vec3f;
        output = mc_MathClosure_86_(in.uv, m);
        mc_MathClosure_86_out = output;
    }
    var mc_MathClosure_89_out: vec4f;
    {
        let levels = mc_MathClosure_86_out;
        let m0 = textureSample(pass_tex_RenderPass_7, pass_samp_RenderPass_7, vec2f((mc_GroupInstance_81_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_81_MathClosure_30_out).y));
        let m1 = textureSample(pass_tex_Downsample_10, pass_samp_Downsample_10, vec2f((mc_GroupInstance_81_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_81_MathClosure_30_out).y));
        let m2 = textureSample(pass_tex_Downsample_12, pass_samp_Downsample_12, vec2f((mc_GroupInstance_81_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_81_MathClosure_30_out).y));
        let m3 = textureSample(pass_tex_Downsample_13, pass_samp_Downsample_13, vec2f((mc_GroupInstance_81_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_81_MathClosure_30_out).y));
        let m4 = textureSample(pass_tex_Downsample_16, pass_samp_Downsample_16, vec2f((mc_GroupInstance_81_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_81_MathClosure_30_out).y));
        let m5 = textureSample(pass_tex_Downsample_18, pass_samp_Downsample_18, vec2f((mc_GroupInstance_81_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_81_MathClosure_30_out).y));
        let m6 = textureSample(pass_tex_Downsample_20, pass_samp_Downsample_20, vec2f((mc_GroupInstance_81_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_81_MathClosure_30_out).y));
        let m7 = textureSample(pass_tex_Downsample_22, pass_samp_Downsample_22, vec2f((mc_GroupInstance_81_MathClosure_30_out).x, 1.0 - (mc_GroupInstance_81_MathClosure_30_out).y));
        var output: vec4f;
        output = mc_MathClosure_89_(in.uv, levels, m0, m1, m2, m3, m4, m5, m6, m7);
        mc_MathClosure_89_out = output;
    }
    return mc_MathClosure_89_out;
}
