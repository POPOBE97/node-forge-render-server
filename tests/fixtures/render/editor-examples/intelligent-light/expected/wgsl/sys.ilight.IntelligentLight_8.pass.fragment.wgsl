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

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let light_radius_px = max(params.target_size.y * 0.5, 1.0);

    var current_color = BASE_COLOR;

    for (var i = 0u; i < NUM_LIGHTS; i = i + 1u) {
        let lpos = ilight_data.lights[i].xy;
        let d = distance(in.local_px.xy, lpos) / light_radius_px;
        let factor = clamp(1.0 - d, 0.0, 1.0);
        let s = smoothstep(0.0, 1.0, factor);
        let light_color = ilight_data.colors[i].xyz * s;
        current_color = current_color * (1.0 - s) + light_color;
    }

    current_color = min(vec3f(1.0), current_color);

    // let power = ilight_data.params.x;
    // let lightness = ilight_data.params.y;
    // let brightness = 1.0 + power * 0.2;
    // let luminance = dot(current_color, vec3f(0.2126, 0.7153, 0.0722));
    // let scale = mix(0.75, 0.775, lightness);
    // let result = mix(vec3f(luminance), current_color, vec3f(brightness)) * scale;

    return vec4f(clamp(current_color, vec3f(0.0), vec3f(1.0)), 1.0);
}
