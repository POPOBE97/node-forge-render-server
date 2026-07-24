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
    base_color: vec4f,
    presentation_colors: array<vec4f, 3>,
    pointer_position_radius_gain: vec4f,
    pointer_color: vec4f,
    pointer_params: vec4f,
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
const LIGHT_COLOR_GAIN: f32 = 1.325;

fn blend_normal(src: vec4f, dst: vec4f) -> vec4f {
    return vec4f(
        src.rgb + dst.rgb * (1.0 - src.a),
        src.a + dst.a * (1.0 - src.a),
    );
}

fn presentation_color(x: f32) -> vec3f {
    if (x < 0.5) {
        return mix(
            srgb_to_linear(max(ilight_data.presentation_colors[0].rgb, vec3f(0.0))),
            srgb_to_linear(max(ilight_data.presentation_colors[1].rgb, vec3f(0.0))),
            smoothstep(0.0, 0.5, x),
        );
    }
    return mix(
        srgb_to_linear(max(ilight_data.presentation_colors[1].rgb, vec3f(0.0))),
        srgb_to_linear(max(ilight_data.presentation_colors[2].rgb, vec3f(0.0))),
        smoothstep(0.5, 1.0, x),
    );
}

fn srgb_to_linear(value: vec3f) -> vec3f {
    let low = value / 12.92;
    let high = pow((value + vec3f(0.055)) / 1.055, vec3f(2.4));
    return select(high, low, value <= vec3f(0.04045));
}

// ── Fragment shader ──────────────────────────────────────────────────
