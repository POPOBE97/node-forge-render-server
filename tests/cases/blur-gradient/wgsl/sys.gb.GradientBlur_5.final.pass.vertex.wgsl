
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
    var uv_2: vec2<f32>;

    uv_1 = uv;
    xy_1 = xy;
    size_1 = size;
    let _e9: vec2<f32> = xy_1;
    let _e10: vec2<f32> = size_1;
    uv_2 = (_e9 / _e10);
    let _e15: vec2<f32> = uv_2;
    let _e19: vec2<f32> = uv_2;
    output = mix(30f, 0f, _e19.y);
    let _e22: f32 = output;
    return _e22;
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


@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
    var out: VSOut;
    out.uv = uv;
    out.geo_size_px = params.geo_size;
    // UV is top-left convention, so flip Y for GLSL-like local_px.
    out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, position.z);

    let p_px = params.center + position.xy;
    out.position = params.camera * vec4f(p_px, position.z, 1.0);
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}
