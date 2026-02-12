pub fn build_image_premultiply_wgsl(tex_var: &str, samp_var: &str) -> String {
    // Convert straight-alpha source -> premultiplied-alpha output.
    // If the source texture is sRGB, textureSample returns linear floats.
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
    // Rendering into an offscreen texture produces a Y-flipped image when later\n\
    // sampled as an ImageTexture (ImageTexture sampling does not flip UVs).\n\
    let uv = nf_uv_pass(in.uv);\n\
    let c = textureSample({tex_var}, {samp_var}, uv);\n\
    return vec4(c.xyz * c.w, c.w);\n\
}}\n"
    )
}

