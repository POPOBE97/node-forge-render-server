pub fn build_srgb_display_encode_wgsl(tex_var: &str, samp_var: &str) -> String {
    // Convert linear scene output -> sRGB-encoded bytes for display paths that treat the
    // framebuffer as linear (common in UI renderers). The source texture is assumed to be an
    // sRGB texture; sampling returns linear floats.
    //
    // Keep alpha linear (do NOT gamma-correct alpha).
    //
    // Also apply the same offscreen-texture Y flip used by PassTexture/premultiply paths so our
    // bottom-left UV convention continues to render upright.
    format!(
        "\
struct Params {{\n\
    target_size: vec2f,\n\
    geo_size: vec2f,\n\
    center: vec2f,\n\
\n\
    geo_translate: vec2f,\n\
    geo_scale: vec2f,\n\
\n\
    time: f32,\n\
    _pad0: f32,\n\
\n\
    color: vec4f,\n\
}};\n\
\n\
@group(0) @binding(0)\n\
var<uniform> params: Params;\n\
\n\
struct VSOut {{\n\
    @builtin(position) position: vec4f,\n\
    @location(0) uv: vec2f,\n\
    @location(1) frag_coord_gl: vec2f,\n\
    @location(2) local_px: vec2f,\n\
    @location(3) geo_size_px: vec2f,\n\
}};\n\
\n\
@group(1) @binding(0)\n\
var {tex_var}: texture_2d<f32>;\n\
\n\
@group(1) @binding(1)\n\
var {samp_var}: sampler;\n\
\n\
fn nf_uv_pass(uv: vec2f) -> vec2f {{\n\
    return vec2f(uv.x, 1.0 - uv.y);\n\
}}\n\
\n\
fn linear_to_srgb_channel(x_in: f32) -> f32 {{\n\
    let x = clamp(x_in, 0.0, 1.0);\n\
    if (x <= 0.0031308) {{\n\
        return x * 12.92;\n\
    }}\n\
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;\n\
}}\n\
\n\
fn linear_to_srgb(rgb: vec3f) -> vec3f {{\n\
    return vec3f(\n\
        linear_to_srgb_channel(rgb.x),\n\
        linear_to_srgb_channel(rgb.y),\n\
        linear_to_srgb_channel(rgb.z),\n\
    );\n\
}}\n\
\n\
@vertex\n\
fn vs_main(\n\
    @location(0) position: vec3f,\n\
    @location(1) uv: vec2f,\n\
) -> VSOut {{\n\
    var out: VSOut;\n\
    let _unused_geo_size = params.geo_size;\n\
    let _unused_geo_translate = params.geo_translate;\n\
    let _unused_geo_scale = params.geo_scale;\n\
\n\
    out.uv = uv;\n\
    out.geo_size_px = params.geo_size;\n\
    out.local_px = uv * out.geo_size_px;\n\
\n\
    let p_local = position;\n\
    let p_px = params.center + p_local.xy;\n\
    let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);\n\
    out.position = vec4f(ndc, position.z, 1.0);\n\
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);\n\
    return out;\n\
}}\n\
\n\
@fragment\n\
fn fs_main(in: VSOut) -> @location(0) vec4f {{\n\
    // Sampling a render-target texture with our bottom-left UV convention requires a Y flip.\n\
    let uv = nf_uv_pass(in.uv);\n\
    let c = textureSample({tex_var}, {samp_var}, uv);\n\
    return vec4f(linear_to_srgb(c.xyz), c.w);\n\
}}\n"
    )
}
