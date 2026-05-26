//── IntelligentLight pass (CPU-driven uniforms) ─────────────────────

struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,
    geo_translate: vec2f,
    geo_scale: vec2f,
    time: f32,
    _pad0: f32,
    color: vec4f,
    camera: mat4x4f,
    camera_position: vec4f,
};

struct ILightData {
    lights: array<vec4f, 11>,
    params: vec4f,
    colors: array<vec4f, 11>,
};

@group(0) @binding(0)
var<uniform> params: Params;
@group(0) @binding(2)
var<uniform> ilight_data: ILightData;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) frag_coord_gl: vec2f,
    @location(2) local_px: vec3f,
    @location(3) geo_size_px: vec2f,
};

@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
    var out: VSOut;

    let _unused_geo_size = params.geo_size;
    let _unused_geo_translate = params.geo_translate;
    let _unused_geo_scale = params.geo_scale;

    out.uv = uv;
    out.geo_size_px = params.geo_size;
    out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, position.z);

    let p_px = params.center + position.xy;
    out.position = params.camera * vec4f(p_px, position.z, 1.0);
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}

// ── Constants ────────────────────────────────────────────────────────

const NUM_LIGHTS: u32 = 11u;
const BASE_COLOR: vec3f = vec3f(0.0, 0.5884, 1.0);

// ── Fragment shader ──────────────────────────────────────────────────
