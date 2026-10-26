
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
    // Node: FloatInput_6ab22c56_6
    node_FloatInput_6ab22c56_6_84bad11b: vec4f,
    // Node: gauge.clock.angle
    node_gauge_clock_angle_d5c8a937: vec4f,
    // Node: gauge.clock.major.base
    node_gauge_clock_major_base_f2347680: vec4f,
    // Node: gauge.clock.major.initial.angle
    node_gauge_clock_major_initial_angle_54a664b1: vec4f,
    // Node: gauge.major.bevel.width
    node_gauge_major_bevel_width_8b9f75cd: vec4f,
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
    let light_uv = in.frag_coord / vec2f(textureDimensions(light));
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

fn nf_math_closure_vertex_closure(uv: vec2<f32>, angle: f32, base: vec3<f32>, index: i32) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var angle_1: f32;
    var base_1: vec3<f32>;
    var index_1: i32;
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
    let _e9: f32 = angle_1;
    let _e10: i32 = index_1;
    let _e15: f32 = angle_1;
    let _e16: i32 = index_1;
    a = radians((_e15 - (f32(_e16) * 30f)));
    let _e24: f32 = angle_1;
    let _e26: i32 = index_1;
    theta = ((150f + _e24) - (f32(_e26) * 30f));
    let _e32: vec3<f32> = base_1;
    v = (_e32.xy - vec2<f32>(1575f, -90f));
    let _e45: f32 = a;
    let _e47: vec2<f32> = v;
    let _e51: f32 = a;
    let _e53: vec2<f32> = v;
    let _e58: f32 = a;
    let _e60: vec2<f32> = v;
    let _e64: f32 = a;
    let _e66: vec2<f32> = v;
    p = (vec2<f32>(1575f, -90f) + vec2<f32>(((cos(_e45) * _e47.x) - (sin(_e51) * _e53.y)), ((sin(_e58) * _e60.x) + (cos(_e64) * _e66.y))));
    let _e73: f32 = theta;
    let _e76: f32 = theta;
    if ((_e73 < 132f) || (_e76 > 168f)) {
        local = vec3<f32>(100000f, 100000f, 0f);
    } else {
        let _e84: vec2<f32> = p;
        let _e85: vec3<f32> = base_1;
        local = vec3<f32>(_e84.x, _e84.y, _e85.z);
    }
    let _e91: vec3<f32> = local;
    output = _e91;
    let _e92: vec3<f32> = output;
    return _e92;
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

 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
     @location(2) i0: vec4f,
     @location(3) i1: vec4f,
     @location(4) i2: vec4f,
     @location(5) i3: vec4f,
     @builtin(instance_index) instance_index: u32,
 ) -> VSOut {
 var out: VSOut;

    let math_closure_out = nf_math_closure_vertex_closure(vec2f(uv.x, 1.0 - uv.y), (graph_inputs.node_gauge_clock_angle_d5c8a937).x, (graph_inputs.node_gauge_clock_major_base_f2347680).xyz, i32(instance_index));
    let math_closure_out_b005e18f = nf_math_closure_vertex_c3173fa0_closure(vec2f(uv.x, 1.0 - uv.y), (graph_inputs.node_gauge_clock_angle_d5c8a937).x, (graph_inputs.node_gauge_clock_major_initial_angle_54a664b1).x, i32(instance_index));
 out.instance_index = instance_index;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = vec2f(uv.x, 1.0 - uv.y);

 let inst_m = mat4x4f(i0, i1, i2, i3);
 let geo_sx = length(inst_m[0].xy);
 let geo_sy = length(inst_m[1].xy);
 let geo_size_px = params.geo_size * vec2f(geo_sx, geo_sy);
 out.geo_size_px = geo_size_px;
 out.local_px = vec3f(uv * geo_size_px, 0.0);

 var p_local = (inst_m * vec4f(position, 1.0)).xyz;

 let delta_t = (sys_apply_trs_xyz(p_local, math_closure_out, math_closure_out_b005e18f, vec3f(1.0, 1.0, 1.0)) - p_local);
 p_local = p_local + delta_t;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = params.center + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }