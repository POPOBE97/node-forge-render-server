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

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let resolution = max(params.target_size, vec2f(1.0));
    let aspect = resolution.x / resolution.y;
    let normalized_coord = clamp(in.local_px.xy / resolution, vec2f(0.0), vec2f(1.0));
    let light_radius = max(ilight_data.params.z / resolution.y, 0.0001);
    let power = clamp(ilight_data.params.x, 0.0, 1.0);
    let opacity = clamp(ilight_data.params.w, 0.0, 1.0);

    var current_color = vec4f(
        srgb_to_linear(max(ilight_data.base_color.rgb, vec3f(0.0)))
            * ilight_data.base_color.a,
        ilight_data.base_color.a,
    );

    for (var i = 0u; i < NUM_LIGHTS; i = i + 1u) {
        let light_position = vec2f(
            ilight_data.lights[i].x / resolution.x,
            1.0 - ilight_data.lights[i].y / resolution.y,
        );
        let scaled_coord = vec2f(normalized_coord.x * aspect, normalized_coord.y);
        let scaled_light_position = vec2f(light_position.x * aspect, light_position.y);
        let distance_ratio =
            distance(scaled_coord, scaled_light_position) / light_radius;
        let blend_amount = smoothstep(
            0.0,
            1.0,
            clamp(1.0 - distance_ratio, 0.0, 1.0),
        );
        let light_color = ilight_data.colors[i];
        let direct_color =
            srgb_to_linear(max(light_color.rgb, vec3f(0.0))) * LIGHT_COLOR_GAIN;
        let presentation =
            presentation_color(light_position.x) * LIGHT_COLOR_GAIN;
        let resolved_rgb = mix(direct_color, presentation, power);
        let resolved_color = vec4f(
            resolved_rgb * light_color.a,
            light_color.a,
        );
        current_color = blend_normal(resolved_color * blend_amount, current_color);
    }

    let pointer_position = ilight_data.pointer_position_radius_gain.xy;
    let pointer_radius = max(ilight_data.pointer_position_radius_gain.z, 1.0);
    let pointer_gain = max(ilight_data.pointer_position_radius_gain.w, 0.0);
    let pointer_delta = in.local_px.xy - pointer_position;
    let pointer_field = exp(
        -4.0 * dot(pointer_delta, pointer_delta) / (pointer_radius * pointer_radius),
    );
    let pointer_alpha = clamp(
        ilight_data.pointer_color.a
            * ilight_data.pointer_params.x
            * pointer_field,
        0.0,
        1.0,
    );
    current_color = vec4f(
        current_color.rgb
            + srgb_to_linear(max(ilight_data.pointer_color.rgb, vec3f(0.0)))
                * pointer_gain
                * pointer_alpha,
        clamp(current_color.a + pointer_alpha, 0.0, 1.0),
    );

    return vec4f(
        max(current_color.rgb * opacity, vec3f(0.0)),
        clamp(current_color.a * opacity, 0.0, 1.0),
    );
}
