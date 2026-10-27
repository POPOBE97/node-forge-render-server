
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
     @location(4) @interpolate(flat) instance_index: u32,
 };


struct GraphInputs {
    // Node: gauge.clock.angle
    node_gauge_clock_angle_d5c8a937: vec4f,
    // Node: gauge.clock.major.base
    node_gauge_clock_major_base_f2347680: vec4f,
    // Node: gauge.clock.major.initial.angle
    node_gauge_clock_major_initial_angle_54a664b1: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_gauge_inlay_composite: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_gauge_inlay_composite: sampler;


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

// The instanced geometry supplies a native cavity SDF. Sample the stationary,
// full-canvas light in that texture's LocalUV, then clip it to the cavity.
fn shader_material_gauge_window_material(
    in: ShaderMaterialInput,
    cavityDistance: f32,
    light: texture_2d<f32>,
    light_sampler: sampler,
) -> vec4f {
    // The vertex varying already includes a half-pixel offset; interpolation
    // supplies pixel-center sampling, so remove that extra offset before lookup.
    let light_uv = (in.frag_coord - vec2f(0.5)) / vec2f(textureDimensions(light));
    let coverage = 1.0 - smoothstep(-0.75, 0.75, cavityDistance);
    return sample_texture_local_uv(light, light_sampler, light_uv) * coverage;
}


// ---- 2D SDF helpers (generated) ----
// 2D SDF helper template.
//
// This file is the editable WGSL source for Sdf2D shape helper functions.
// The Rust compiler wires node inputs into calls to these helpers.

fn sdf2d_round_rect(p: vec2f, b: vec2f, rad4: vec4f) -> f32 {
    var r: f32 = rad4.x;
    if (p.x > 0.0 && p.y > 0.0) {
        r = rad4.y;
    } else if (p.x > 0.0 && p.y < 0.0) {
        r = rad4.z;
    } else if (p.x < 0.0 && p.y < 0.0) {
        r = rad4.w;
    }

    let q = abs(p) - b + vec2f(r, r);
    let outside = length(max(q, vec2f(0.0, 0.0)));
    let inside = min(max(q.x, q.y), 0.0);
    return outside + inside - r;
}

fn sdf2d_smooth_round_rect(point: vec2f, center: vec2f, radius: f32, axis_mix: vec2f) -> vec3f {
    let abs_radius = abs(radius);
    let scaled_radius = 1.5286649465560913 * abs_radius;
    let safe_scaled_radius = max(scaled_radius, 1e-6);
    let blended_radius = mix(scaled_radius, radius, max(axis_mix.x, axis_mix.y));

    let offset = point - center;
    let shifted_pos = vec2f(safe_scaled_radius, safe_scaled_radius) + offset;
    let normalized_pos = max(vec2f(0.0), shifted_pos / safe_scaled_radius);
    let abs_norm_pos = abs(normalized_pos);

    let axis_denom = max(abs_norm_pos.x, abs_norm_pos.y);
    let axis_ratio = select(
        clamp(min(abs_norm_pos.x, abs_norm_pos.y) / axis_denom, 0.0, 1.0),
        0.0,
        axis_denom == 0.0,
    );

    let poly_fit_0 = -0.7391197269 * axis_ratio + 2.4034927648;
    let poly_fit_1 = poly_fit_0 * axis_ratio - 2.4907319173;
    let poly_fit_2 = poly_fit_1 * axis_ratio + 0.4768708960;
    let poly_fit = poly_fit_2 * axis_ratio + 0.4747847594;
    let len_abs = length(abs_norm_pos);
    let denom = 1.0 - axis_ratio * axis_ratio * clamp(len_abs, 0.0, 1.0) * poly_fit;
    let safe_denom = select(denom, 1e-6, abs(denom) < 1e-6);
    let dist_base = (len_abs + 1.0) - 1.0 / safe_denom;
    let dist_alt_pos = max(
        vec2f(0.0),
        1.5286649465560913 * abs_norm_pos - vec2f(0.5286650061607361),
    );
    let dist_alt = 0.6541655659675598 * length(dist_alt_pos) + 0.3458344340324402;

    let dist_mix_x = mix(dist_base, dist_alt, axis_mix.x);
    let dist_mix_y = mix(dist_base, dist_alt, axis_mix.y);
    let axis_sign = select(-1.0, 1.0, abs_norm_pos.y > abs_norm_pos.x);
    let final_mix = mix(dist_mix_x, dist_mix_y, clamp(0.5 - axis_sign + axis_sign * axis_ratio, 0.0, 1.0));

    let radial_pos = vec2f(blended_radius, blended_radius) + offset;
    let dir_norm = normalize(max(vec2f(0.0), radial_pos));
    let fallback_axis = select(vec2f(0.0, 1.0), vec2f(1.0, 0.0), radial_pos.x > radial_pos.y);
    let fallback_dir = select(fallback_axis, dir_norm, dir_norm.x + dir_norm.y > 0.0);
    let final_height = min(max(radial_pos.x, radial_pos.y), 0.0) + safe_scaled_radius * (final_mix - 1.0);

    return vec3f(final_height, fallback_dir);
}


// --- Extra WGSL declarations (generated) ---
fn nf_math_closure_vertex_c3173fa0_closure(uv: vec2<f32>, angle: f32, initialAngle: f32, index: i32) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var angle_1: f32;
    var initialAngle_1: f32;
    var index_1: i32;
    var output: vec3<f32>;

    uv_1 = uv;
    angle_1 = angle;
    initialAngle_1 = initialAngle;
    index_1 = index;
    let _e11: f32 = initialAngle_1;
    let _e12: f32 = angle_1;
    let _e14: i32 = index_1;
    output = vec3<f32>(0f, 0f, ((_e11 + _e12) - (f32(_e14) * 30f)));
    let _e20: vec3<f32> = output;
    return _e20;
}

fn nf_math_closure_vertex_closure(uv: vec2<f32>, angle: f32, base: vec3<f32>, index: i32, dialCenter: vec2<f32>) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var angle_1: f32;
    var base_1: vec3<f32>;
    var index_1: i32;
    var dialCenter_1: vec2<f32>;
    var output: vec3<f32>;
    var a: f32;
    var theta: f32;
    var v: vec2<f32>;
    var p: vec2<f32>;
    var local: vec3<f32>;

    uv_1 = uv;
    angle_1 = angle;
    base_1 = base;
    index_1 = index;
    dialCenter_1 = dialCenter;
    let _e11: f32 = angle_1;
    let _e12: i32 = index_1;
    let _e17: f32 = angle_1;
    let _e18: i32 = index_1;
    a = radians((_e17 - (f32(_e18) * 30f)));
    let _e26: f32 = angle_1;
    let _e28: i32 = index_1;
    theta = ((150f + _e26) - (f32(_e28) * 30f));
    let _e34: vec3<f32> = base_1;
    let _e36: vec2<f32> = dialCenter_1;
    v = (_e34.xy - _e36);
    let _e39: vec2<f32> = dialCenter_1;
    let _e41: f32 = a;
    let _e43: vec2<f32> = v;
    let _e47: f32 = a;
    let _e49: vec2<f32> = v;
    let _e54: f32 = a;
    let _e56: vec2<f32> = v;
    let _e60: f32 = a;
    let _e62: vec2<f32> = v;
    p = (_e39 + vec2<f32>(((cos(_e41) * _e43.x) - (sin(_e47) * _e49.y)), ((sin(_e54) * _e56.x) + (cos(_e60) * _e62.y))));
    let _e69: f32 = theta;
    let _e72: f32 = theta;
    if ((_e69 < 132f) || (_e72 > 168f)) {
        local = vec3<f32>(100000f, 100000f, 0f);
    } else {
        let _e80: vec2<f32> = p;
        let _e81: vec3<f32> = base_1;
        local = vec3<f32>(_e80.x, _e80.y, _e81.z);
    }
    let _e87: vec3<f32> = local;
    output = _e87;
    let _e88: vec3<f32> = output;
    return _e88;
}


fn sys_apply_trs_xyz(p: vec3f, t: vec3f, r_deg: vec3f, s: vec3f) -> vec3f {
    let rad = r_deg * 0.017453292519943295;

    let cx = cos(rad.x);
    let sx = sin(rad.x);
    let cy = cos(rad.y);
    let sy = sin(rad.y);
    let cz = cos(rad.z);
    let sz = sin(rad.z);

    let p0 = p * s;
    let p1 = vec3f(p0.x, p0.y * cx - p0.z * sx, p0.y * sx + p0.z * cx);
    let p2 = vec3f(p1.x * cy + p1.z * sy, p1.y, -p1.x * sy + p1.z * cy);
    let p3 = vec3f(p2.x * cz - p2.y * sz, p2.x * sz + p2.y * cz, p2.z);
    return p3 + t;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Add gauge.major.inner.result
    let math_add = (
        sdf2d_smooth_round_rect(abs((in.local_px.xy - vec2f(143.5, 40))), (vec2f(279, 72) * 0.5), 36, vec2f(1, 1)).x
        + (8.0 + 8.0)
    );
    // Shader Material gauge.window.material.material
    let shader_material_material = shader_material_gauge_window_material(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        math_add,
        pass_tex_gauge_inlay_composite,
        pass_samp_gauge_inlay_composite,
    );
    // Final composite
    let _frag_out = shader_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
