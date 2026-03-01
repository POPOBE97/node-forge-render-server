//! Core structs for shader-space pass assembly.
//!
//! This module holds the internal data types used to describe render-pass
//! specifications before they are registered with `ShaderSpace`.

use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color, TextureFormat},
};

use crate::renderer::types::{GraphBinding, Params};

// Re-export the `PassTextureBinding` canonical definition from render_plan.
pub(crate) type PassTextureBinding = crate::renderer::render_plan::types::PassTextureBinding;

/// Sampler configuration picked per texture binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SamplerKind {
    NearestClamp,
    NearestMirror,
    NearestRepeat,
    LinearMirror,
    LinearRepeat,
    LinearClamp,
}

/// Texture declaration for the assembly phase (before `ShaderSpace::declare_textures`).
#[derive(Clone)]
pub(crate) struct TextureDecl {
    pub name: ResourceName,
    pub size: [u32; 2],
    pub format: TextureFormat,
    pub sample_count: u32,
    /// When true, include `TEXTURE_BINDING` even for multi-sampled textures
    /// (e.g. depth attachments read by a depth-resolve pass via `textureLoad`).
    pub needs_sampling: bool,
}

/// Complete specification for a single render pass, ready to be registered.
#[derive(Clone)]
pub(crate) struct RenderPassSpec {
    pub pass_id: String,
    pub name: ResourceName,
    pub geometry_buffer: ResourceName,
    pub instance_buffer: Option<ResourceName>,
    pub normals_buffer: Option<ResourceName>,
    pub target_texture: ResourceName,
    pub resolve_target: Option<ResourceName>,
    pub params_buffer: ResourceName,
    pub baked_data_parse_buffer: Option<ResourceName>,
    pub params: Params,
    pub graph_binding: Option<GraphBinding>,
    pub graph_values: Option<Vec<u8>>,
    pub shader_wgsl: String,
    pub texture_bindings: Vec<PassTextureBinding>,
    pub sampler_kinds: Vec<SamplerKind>,
    pub blend_state: BlendState,
    pub color_load_op: wgpu::LoadOp<Color>,
    pub sample_count: u32,
}

/// Depth-resolve pass: reads a `Depth32Float` attachment and writes a regular
/// colour texture so that downstream consumers can sample it via `texture_2d<f32>`.
#[derive(Clone)]
pub(crate) struct DepthResolvePass {
    pub pass_name: ResourceName,
    pub geometry_buffer: ResourceName,
    pub params_buffer: ResourceName,
    pub params: Params,
    pub depth_texture: ResourceName,
    pub dst_texture: ResourceName,
    pub shader_wgsl: String,
    pub is_multisampled: bool,
}

/// GPU prepass that converts a straight-alpha source image into a premultiplied destination.
#[derive(Clone)]
pub(crate) struct ImagePrepass {
    pub pass_name: ResourceName,
    pub geometry_buffer: ResourceName,
    pub params_buffer: ResourceName,
    pub params: Params,
    pub src_texture: ResourceName,
    pub dst_texture: ResourceName,
    pub shader_wgsl: String,
}

/// Texture capability requirement for validation.
#[derive(Clone, Debug)]
pub(crate) struct TextureCapabilityRequirement {
    pub name: ResourceName,
    pub format: TextureFormat,
    pub usage: wgpu::TextureUsages,
    pub sample_count: u32,
    pub sampled_by_passes: Vec<String>,
    pub blend_target_passes: Vec<String>,
}

// ── Helpers ──────────────────────────────────────────────────────────────

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
    // For both types textureLoad takes (tex, coord, sample_or_level) where the
    // third argument is a sample index (multisampled) or mip level (non-ms).
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
