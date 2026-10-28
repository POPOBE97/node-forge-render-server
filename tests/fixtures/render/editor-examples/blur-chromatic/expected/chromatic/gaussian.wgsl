
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
    // Node: dp.mask.gaussianMask
    dp_mask_gaussian_mask: vec4f,
    // Node: effect.center
    effect_center: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;
@group(1) @binding(0)
var pass_tex_sys_gb_chromatic_blur_pad: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_sys_gb_chromatic_blur_pad: sampler;

@group(1) @binding(2)
var pass_tex_sys_gb_chromatic_blur_mip1: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_sys_gb_chromatic_blur_mip1: sampler;

@group(1) @binding(4)
var pass_tex_sys_gb_chromatic_blur_mip2: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp_sys_gb_chromatic_blur_mip2: sampler;

@group(1) @binding(6)
var pass_tex_sys_gb_chromatic_blur_mip3: texture_2d<f32>;

@group(1) @binding(7)
var pass_samp_sys_gb_chromatic_blur_mip3: sampler;

@group(1) @binding(8)
var pass_tex_sys_gb_chromatic_blur_mip4: texture_2d<f32>;

@group(1) @binding(9)
var pass_samp_sys_gb_chromatic_blur_mip4: sampler;

@group(1) @binding(10)
var pass_tex_sys_gb_chromatic_blur_mip5: texture_2d<f32>;

@group(1) @binding(11)
var pass_samp_sys_gb_chromatic_blur_mip5: sampler;

@group(1) @binding(12)
var pass_tex_sys_gb_chromatic_blur_mip6: texture_2d<f32>;

@group(1) @binding(13)
var pass_samp_sys_gb_chromatic_blur_mip6: sampler;

@group(1) @binding(14)
var pass_tex_sys_cb_chromatic_blur_rgb: texture_2d<f32>;

@group(1) @binding(15)
var pass_samp_sys_cb_chromatic_blur_rgb: sampler;


// --- Extra WGSL declarations (generated) ---
fn nf_math_closure_fragment_closure(uv: vec2<f32>, size: vec2<f32>, center: vec2<f32>, shape: vec4<f32>, xy: vec2<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var size_1: vec2<f32>;
    var center_1: vec2<f32>;
    var shape_1: vec4<f32>;
    var xy_1: vec2<f32>;
    var output: f32;
    var distance: f32;
    var outer: f32;
    var inner: f32;
    var outside: f32;
    var local: f32;
    var local_1: f32;

    uv_1 = uv;
    size_1 = size;
    center_1 = center;
    shape_1 = shape;
    xy_1 = xy;
    let _e11: vec2<f32> = xy_1;
    let _e12: vec2<f32> = center_1;
    let _e14: vec2<f32> = xy_1;
    let _e15: vec2<f32> = center_1;
    distance = length((_e14 - _e15));
    let _e19: vec4<f32> = shape_1;
    let _e22: vec4<f32> = shape_1;
    outer = max(_e22.x, 1f);
    let _e27: f32 = outer;
    let _e29: vec4<f32> = shape_1;
    let _e33: vec4<f32> = shape_1;
    inner = (_e27 * (1f - clamp(_e33.y, 0.001f, 1f)));
    let _e44: f32 = inner;
    let _e45: f32 = outer;
    let _e46: f32 = distance;
    outside = smoothstep(_e44, _e45, _e46);
    let _e49: vec4<f32> = shape_1;
    if (_e49.w > 0.5f) {
        let _e53: f32 = outside;
        local = _e53;
    } else {
        let _e55: f32 = outside;
        local = (1f - _e55);
    }
    let _e59: vec4<f32> = shape_1;
    let _e62: vec4<f32> = shape_1;
    let _e66: vec4<f32> = shape_1;
    if (_e66.w > 0.5f) {
        let _e70: f32 = outside;
        local_1 = _e70;
    } else {
        let _e72: f32 = outside;
        local_1 = (1f - _e72);
    }
    let _e75: f32 = local_1;
    let _e76: vec4<f32> = shape_1;
    let _e79: vec4<f32> = shape_1;
    output = pow(_e75, max(_e79.z, 0.01f));
    let _e84: f32 = output;
    return _e84;
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
    let uv = vec2f(xy.x, resolution.y - xy.y) / resolution;
    if (level == 0) {
        return textureSampleLevel(pass_tex_sys_gb_chromatic_blur_pad, pass_samp_sys_gb_chromatic_blur_pad, uv, 0.0);
    } else if (level == 1) {
        return textureSampleLevel(pass_tex_sys_gb_chromatic_blur_mip1, pass_samp_sys_gb_chromatic_blur_mip1, uv, 0.0);
    } else if (level == 2) {
        return textureSampleLevel(pass_tex_sys_gb_chromatic_blur_mip2, pass_samp_sys_gb_chromatic_blur_mip2, uv, 0.0);
    } else if (level == 3) {
        return textureSampleLevel(pass_tex_sys_gb_chromatic_blur_mip3, pass_samp_sys_gb_chromatic_blur_mip3, uv, 0.0);
    } else if (level == 4) {
        return textureSampleLevel(pass_tex_sys_gb_chromatic_blur_mip4, pass_samp_sys_gb_chromatic_blur_mip4, uv, 0.0);
    } else if (level == 5) {
        return textureSampleLevel(pass_tex_sys_gb_chromatic_blur_mip5, pass_samp_sys_gb_chromatic_blur_mip5, uv, 0.0);
    } else if (level == 6) {
        return textureSampleLevel(pass_tex_sys_gb_chromatic_blur_mip6, pass_samp_sys_gb_chromatic_blur_mip6, uv, 0.0);
    }
    return vec4f(0.0);
}


@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
    var out: VSOut;
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    out.geo_size_px = params.geo_size;
    // UV is top-left convention, so flip Y for GLSL-like local_px.
    out.local_px = vec3f(uv * out.geo_size_px, position.z);

    let p_px = params.center + position.xy;
    out.position = params.camera * vec4f(p_px, position.z, 1.0);
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        let math_closure_out = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), in.geo_size_px, (graph_inputs.effect_center).xy, (graph_inputs.dp_mask_gaussian_mask), in.local_px.xy);
// Evaluate mask → sigma in pixels.
    // NOTE: The mask expression sees user coordinates (in.local_px),
    // i.e. (0,0) = bottom-left of the original source image.
    let gb_sigma = max(select(0.0, max(13.333333, 0.0) * clamp(math_closure_out, 0.0, 1.0), true), 0.0);
    if (gb_sigma <= 0.0) { return textureSampleLevel(pass_tex_sys_cb_chromatic_blur_rgb, pass_samp_sys_cb_chromatic_blur_rgb, in.uv, 0.0); }

    // Sigma → mip level (clamped to safe range).
    var gb_m: f32 = 0.0;
    if (gb_sigma > 0.0) {
        gb_m = clamp(log2(gb_sigma * 1.333333), 0.0, 6.0);
    }

    let gb_mip0_size = vec2f(1152.0, 1152.0);

    // Transform from user coordinates (original image space) to padded
    // texture coordinates.  User (0,0) → padded (pad_offset).
    let gb_pad_offset = vec2f(36.0, 36.0);
    let gb_coord = in.local_px.xy + gb_pad_offset;

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
