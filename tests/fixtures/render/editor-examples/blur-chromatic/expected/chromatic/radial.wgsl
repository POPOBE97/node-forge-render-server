
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
    // Node: dp.mask.radialMask
    dp_mask_radial_mask: vec4f,
    // Node: effect.center
    effect_center: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;
@group(1) @binding(0)
var pass_tex_sys_cb_chromatic_blur_input: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_sys_cb_chromatic_blur_input: sampler;


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


fn cb_read(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f) -> vec4f {
    return sample_texture_local_uv(image, image_sampler, uv);
}

fn cb_radial(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f,
    center_px: vec2f, size: vec2f, amount: f32, samples: i32) -> vec4f {
    let count = u32(clamp(samples, 2, 48));
    let ray = (uv * size - center_px) / size;
    var result = vec4f(0.0);
    for (var i = 0u; i < count; i++) {
        let t = f32(i) / f32(count - 1u) - 0.5;
        result += cb_read(image, image_sampler, uv + ray * t * amount);
    }
    return result / f32(count);
}

fn cb_rgb(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f,
    center_px: vec2f, size: vec2f, amount: f32, offset: f32, spread: f32,
    samples: i32, channels: vec3f, fixed_direction: bool, angle_degrees: f32) -> vec4f {
    let delta = uv * size - center_px;
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

fn nf_math_closure_fragment_closure(uv: vec2<f32>, size: vec2<f32>, center: vec2<f32>, shape: vec4<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var size_1: vec2<f32>;
    var center_1: vec2<f32>;
    var shape_1: vec4<f32>;
    var output: f32;
    var distance: f32;
    var outer: f32;
    var inner: f32;
    var outside: f32;
    var local: f32;
    var local_1: f32;

    uv_1 = uv;
    size_1 = size;
    center_1 = center;
    shape_1 = shape;
    let _e9: vec2<f32> = uv_1;
    let _e10: vec2<f32> = size_1;
    let _e12: vec2<f32> = center_1;
    let _e14: vec2<f32> = uv_1;
    let _e15: vec2<f32> = size_1;
    let _e17: vec2<f32> = center_1;
    distance = length(((_e14 * _e15) - _e17));
    let _e21: vec4<f32> = shape_1;
    let _e24: vec4<f32> = shape_1;
    outer = max(_e24.x, 1f);
    let _e29: f32 = outer;
    let _e31: vec4<f32> = shape_1;
    let _e35: vec4<f32> = shape_1;
    inner = (_e29 * (1f - clamp(_e35.y, 0.001f, 1f)));
    let _e46: f32 = inner;
    let _e47: f32 = outer;
    let _e48: f32 = distance;
    outside = smoothstep(_e46, _e47, _e48);
    let _e51: vec4<f32> = shape_1;
    if (_e51.w > 0.5f) {
        let _e55: f32 = outside;
        local = _e55;
    } else {
        let _e57: f32 = outside;
        local = (1f - _e57);
    }
    let _e61: vec4<f32> = shape_1;
    let _e64: vec4<f32> = shape_1;
    let _e68: vec4<f32> = shape_1;
    if (_e68.w > 0.5f) {
        let _e72: f32 = outside;
        local_1 = _e72;
    } else {
        let _e74: f32 = outside;
        local_1 = (1f - _e74);
    }
    let _e77: f32 = local_1;
    let _e78: vec4<f32> = shape_1;
    let _e81: vec4<f32> = shape_1;
    output = pow(_e77, max(_e81.z, 0.01f));
    let _e86: f32 = output;
    return _e86;
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
    let math_closure_out = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), in.geo_size_px, (graph_inputs.effect_center).xy, (graph_inputs.dp_mask_radial_mask));
    // Final composite
    let _frag_out = cb_radial(pass_tex_sys_cb_chromatic_blur_input, pass_samp_sys_cb_chromatic_blur_input, vec2f(in.uv.x, 1.0 - in.uv.y), (graph_inputs.effect_center).xy, params.geo_size, select(0.0, clamp(math_closure_out, 0.0, 1.0), true) * 0.4, 8);
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
