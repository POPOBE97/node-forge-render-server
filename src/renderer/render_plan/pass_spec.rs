//! Shared pass-planning helper utilities.

pub(crate) use crate::renderer::render_plan::types::{
    DepthResolvePass, ImagePrepass, PassTextureBinding, RenderPassSpec, SamplerKind,
    TextureCapabilityRequirement, TextureDecl,
};

use crate::renderer::types::Params;

pub(crate) const IDENTITY_MAT4: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

pub(crate) fn make_params(
    target_size: [f32; 2],
    geo_size: [f32; 2],
    center: [f32; 2],
    camera: [f32; 16],
    color: [f32; 4],
) -> Params {
    Params {
        target_size,
        geo_size,
        center,
        geo_translate: [0.0, 0.0],
        geo_scale: [1.0, 1.0],
        time: 0.0,
        _pad0: 0.0,
        color,
        camera,
    }
}

pub(crate) fn build_depth_resolve_wgsl(multisampled: bool) -> String {
    let depth_tex_type = if multisampled {
        "texture_depth_multisampled_2d"
    } else {
        "texture_depth_2d"
    };
    let load_arg = "0";
    format!(
        r#"struct Params {{
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,
    geo_translate: vec2f,
    geo_scale: vec2f,
    time: f32,
    _pad0: f32,
    color: vec4f,
    camera: mat4x4f,
}};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {{
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) frag_coord_gl: vec2f,
    @location(2) local_px: vec3f,
    @location(3) geo_size_px: vec2f,
}};

@group(1) @binding(0)
var depth_tex: {depth_tex_type};

@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {{
    var out: VSOut;
    out.uv = uv;
    out.geo_size_px = params.geo_size;
    out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, position.z);
    let p_px = params.center + position.xy;
    out.position = params.camera * vec4f(p_px, position.z, 1.0);
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {{
    let coord = vec2<i32>(in.position.xy);
    let d = textureLoad(depth_tex, coord, {load_arg});
    return vec4f(d, d, d, 1.0);
}}"#
    )
}
