
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
    // Node: dp.chromatic.blur.rgbOffset
    float_input_dp: vec4f,
    // Node: dp.chromatic.blur.rgbSpread
    float_input_dp_b6947df2: vec4f,
    // Node: dp.mask.rgbMask
    dp_mask_rgb_mask: vec4f,
    // Node: effect.center
    effect_center: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;
@group(1) @binding(0)
var pass_tex_sys_cb_chromatic_blur_radial: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_sys_cb_chromatic_blur_radial: sampler;


// --- Extra WGSL declarations (generated) ---

struct ShaderMaterialInput {
    // Public material UV: bottom-left origin, Y increasing upward.
    uv: vec2f,
    // Public scene/target pixel coordinate: bottom-left origin, Y increasing upward.
    frag_coord: vec2f,
    // Public geometry-local pixel coordinate: bottom-left origin, Y increasing upward.
    local_position: vec3f,
    geometry_size: vec2f,
    target_size: vec2f,
    time: f32,
};

// Renderer-owned texture boundary. ShaderMaterial authors provide LocalUV and never
// convert to WebGPU's private raster-texture convention themselves.
fn sample_texture_local_uv(
    source: texture_2d<f32>,
    source_sampler: sampler,
    local_uv: vec2f,
) -> vec4f {
    return textureSample(source, source_sampler, vec2f(local_uv.x, 1.0 - local_uv.y));
}


fn cb_mask(uv: vec2f, center: vec2f, size: vec2f, mask: vec4f) -> f32 {
    let distance = length((uv - center) * size);
    let outer = max(mask.x, 1.0);
    let inner = outer * (1.0 - clamp(mask.y, 0.001, 1.0));
    let outside = smoothstep(inner, outer, distance);
    return pow(select(1.0 - outside, outside, mask.w > 0.5), max(mask.z, 0.01));
}


fn cb_read(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f) -> vec4f {
    let value = sample_texture_local_uv(image, image_sampler, uv);
    return select(value, vec4f(0.0), any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)));
}

fn cb_radial(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f,
    center: vec2f, amount: f32, samples: i32) -> vec4f {
    let count = u32(clamp(samples, 2, 48));
    let ray = uv - center;
    var result = vec4f(0.0);
    for (var i = 0u; i < count; i++) {
        let t = f32(i) / f32(count - 1u) - 0.5;
        result += cb_read(image, image_sampler, uv + ray * t * amount);
    }
    return result / f32(count);
}

fn cb_rgb(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f,
    center: vec2f, size: vec2f, amount: f32, offset: f32, spread: f32,
    samples: i32, channels: vec3f, fixed_direction: bool, angle_degrees: f32) -> vec4f {
    let delta = (uv - center) * size;
    let angle = angle_degrees * 0.017453292519943295;
    let direction = select(delta / max(length(delta), 0.0001), vec2f(cos(angle), sin(angle)), fixed_direction);
    let step = direction * amount / size;
    let count = u32(clamp(samples, 1, 16));
    var result = vec3f(0.0);
    var weights = 0.0;
    for (var i = 0u; i < count; i++) {
        let t = select(f32(i) / f32(max(count - 1u, 1u)) * 2.0 - 1.0, 0.0, count == 1u);
        let weight = exp(-2.0 * t * t);
        result += vec3f(
            cb_read(image, image_sampler, uv + step * (offset * channels.x + t * spread)).r,
            cb_read(image, image_sampler, uv + step * (offset * channels.y + t * spread)).g,
            cb_read(image, image_sampler, uv + step * (offset * channels.z + t * spread)).b
        ) * weight;
        weights += weight;
    }
    return vec4f(result / weights, cb_read(image, image_sampler, uv).a);
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
    // Final composite
    let _frag_out = cb_rgb(pass_tex_sys_cb_chromatic_blur_radial, pass_samp_sys_cb_chromatic_blur_radial, vec2f(in.uv.x, 1.0 - in.uv.y), (graph_inputs.effect_center).xy, params.geo_size, select(0.0, cb_mask(vec2f(in.uv.x, 1.0 - in.uv.y), (graph_inputs.effect_center).xy, params.geo_size, (graph_inputs.dp_mask_rgb_mask)), true), (graph_inputs.float_input_dp).x, max((graph_inputs.float_input_dp_b6947df2).x, 0.0), 4, vec3f(1.0, 0.0, -1.0), false, 0.0);
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
