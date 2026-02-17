
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

@group(1) @binding(0)
var pass_tex_sys_gb_GradientBlur_5_pad: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_sys_gb_GradientBlur_5_pad: sampler;

@group(1) @binding(2)
var pass_tex_sys_gb_GradientBlur_5_mip1: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_sys_gb_GradientBlur_5_mip1: sampler;

@group(1) @binding(4)
var pass_tex_sys_gb_GradientBlur_5_mip2: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp_sys_gb_GradientBlur_5_mip2: sampler;

@group(1) @binding(6)
var pass_tex_sys_gb_GradientBlur_5_mip3: texture_2d<f32>;

@group(1) @binding(7)
var pass_samp_sys_gb_GradientBlur_5_mip3: sampler;

@group(1) @binding(8)
var pass_tex_sys_gb_GradientBlur_5_mip4: texture_2d<f32>;

@group(1) @binding(9)
var pass_samp_sys_gb_GradientBlur_5_mip4: sampler;

@group(1) @binding(10)
var pass_tex_sys_gb_GradientBlur_5_mip5: texture_2d<f32>;

@group(1) @binding(11)
var pass_samp_sys_gb_GradientBlur_5_mip5: sampler;

@group(1) @binding(12)
var pass_tex_sys_gb_GradientBlur_5_mip6: texture_2d<f32>;

@group(1) @binding(13)
var pass_samp_sys_gb_GradientBlur_5_mip6: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_MathClosure_7_(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var output: f32 = 0f;
    var pStart: vec3<f32> = vec3<f32>(0f, 48f, 30f);
    var pEnd: vec3<f32> = vec3<f32>(0f, 2400f, 0f);
    var qBase: vec2<f32>;
    var md: f32;
    var q: vec2<f32>;
    var p: f32;
    var m: f32;

    uv_1 = uv;
    xy_1 = xy;
    size_1 = size;
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
    output = (_e50.z + ((_e52.z - _e54.z) * _e57));
    let _e60: f32 = output;
    return _e60;
}


// --- GradientBlur composite helpers (generated) ---

fn gb_mvb_up(dc: vec2f, scale: f32) -> array<vec2f, 4> {
    let d     = dc * scale - 0.5;
    let c     = floor(d);
    let x     = c - d + 1.0;
    let X     = d - c;
    let x3    = x * x * x;
    let coeff = 0.5 * x * x + 0.5 * x + 0.166667;
    let w1    = -0.333333 * x3 + coeff;
    let w2    = 1.0 - w1;
    let o1    = (-0.5 * x3 + coeff) / w1 + c - 0.5;
    let o2    = (X * X * X / 6.0) / w2 + c + 1.5;
    return array<vec2f, 4>(w1, w2, o1, o2);
}

fn gb_sample_from_mipmap(xy: vec2f, resolution: vec2f, level: i32) -> vec4f {
    let uv = xy / resolution;
    if (level == 0) {
        return textureSampleLevel(pass_tex_sys_gb_GradientBlur_5_pad, pass_samp_sys_gb_GradientBlur_5_pad, uv, 0.0);
    } else if (level == 1) {
        return textureSampleLevel(pass_tex_sys_gb_GradientBlur_5_mip1, pass_samp_sys_gb_GradientBlur_5_mip1, uv, 0.0);
    } else if (level == 2) {
        return textureSampleLevel(pass_tex_sys_gb_GradientBlur_5_mip2, pass_samp_sys_gb_GradientBlur_5_mip2, uv, 0.0);
    } else if (level == 3) {
        return textureSampleLevel(pass_tex_sys_gb_GradientBlur_5_mip3, pass_samp_sys_gb_GradientBlur_5_mip3, uv, 0.0);
    } else if (level == 4) {
        return textureSampleLevel(pass_tex_sys_gb_GradientBlur_5_mip4, pass_samp_sys_gb_GradientBlur_5_mip4, uv, 0.0);
    } else if (level == 5) {
        return textureSampleLevel(pass_tex_sys_gb_GradientBlur_5_mip5, pass_samp_sys_gb_GradientBlur_5_mip5, uv, 0.0);
    } else if (level == 6) {
        return textureSampleLevel(pass_tex_sys_gb_GradientBlur_5_mip6, pass_samp_sys_gb_GradientBlur_5_mip6, uv, 0.0);
    }
    return vec4f(0.0);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_7_out: f32;
    {
        let xy = in.local_px;
        let size = in.geo_size_px;
        var output: f32;
        output = mc_MathClosure_7_(in.uv, xy, size);
        mc_MathClosure_7_out = output;
    }
// Evaluate mask → sigma in pixels.
    // NOTE: The mask expression sees user coordinates (in.local_px),
    // i.e. (0,0) = bottom-left of the original source image.
    let gb_sigma = max(mc_MathClosure_7_out, 0.0);

    // Sigma → mip level (clamped to safe range).
    var gb_m: f32 = 0.0;
    if (gb_sigma > 0.0) {
        gb_m = clamp(log2(gb_sigma * 1.333333), 0.0, 6.0);
    }

    let gb_mip0_size = vec2f(1152.0, 2496.0);

    // Use local_px directly as coordinate into padded mip textures.
    // The pad pass stores the source upside-down so that
    // `uv = local_px / resolution` naturally maps bottom-left screen
    // to the correct content, matching the graph-based mipmap-blend
    // reference.
    let gb_coord = in.local_px;

    // Floor / ceil mip levels.
    let gb_mLo = floor(gb_m);
    var gb_cLo: vec4f;

    if (gb_mLo < 0.1) {
        gb_cLo = gb_sample_from_mipmap(gb_coord, gb_mip0_size, 0);
    } else {
        let gb_scale_lo = 1.0 / pow(2.0, gb_mLo);
        let gb_lo_res = gb_mip0_size / pow(2.0, gb_mLo);
        let gb_w_lo = gb_mvb_up(gb_coord, gb_scale_lo);
        gb_cLo = gb_w_lo[0].x * gb_w_lo[0].y * gb_sample_from_mipmap(vec2f(gb_w_lo[2].x, gb_w_lo[2].y), gb_lo_res, i32(gb_mLo))
                + gb_w_lo[1].x * gb_w_lo[0].y * gb_sample_from_mipmap(vec2f(gb_w_lo[3].x, gb_w_lo[2].y), gb_lo_res, i32(gb_mLo))
                + gb_w_lo[0].x * gb_w_lo[1].y * gb_sample_from_mipmap(vec2f(gb_w_lo[2].x, gb_w_lo[3].y), gb_lo_res, i32(gb_mLo))
                + gb_w_lo[1].x * gb_w_lo[1].y * gb_sample_from_mipmap(vec2f(gb_w_lo[3].x, gb_w_lo[3].y), gb_lo_res, i32(gb_mLo));
    }

    let gb_mHi = gb_mLo + 1.0;
    let gb_scale_hi = 1.0 / pow(2.0, gb_mHi);
    let gb_hi_res = gb_mip0_size / pow(2.0, gb_mHi);
    let gb_w_hi = gb_mvb_up(gb_coord, gb_scale_hi);
    let gb_cHi = gb_w_hi[0].x * gb_w_hi[0].y * gb_sample_from_mipmap(vec2f(gb_w_hi[2].x, gb_w_hi[2].y), gb_hi_res, i32(gb_mHi))
               + gb_w_hi[1].x * gb_w_hi[0].y * gb_sample_from_mipmap(vec2f(gb_w_hi[3].x, gb_w_hi[2].y), gb_hi_res, i32(gb_mHi))
               + gb_w_hi[0].x * gb_w_hi[1].y * gb_sample_from_mipmap(vec2f(gb_w_hi[2].x, gb_w_hi[3].y), gb_hi_res, i32(gb_mHi))
               + gb_w_hi[1].x * gb_w_hi[1].y * gb_sample_from_mipmap(vec2f(gb_w_hi[3].x, gb_w_hi[3].y), gb_hi_res, i32(gb_mHi));

    return mix(gb_cLo, gb_cHi, gb_m - gb_mLo);
}
