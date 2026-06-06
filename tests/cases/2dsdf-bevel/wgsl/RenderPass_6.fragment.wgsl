
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
    // Node: FloatInput_10
    float_input_10: vec4f,
    // Node: FloatInput_12
    float_input_12: vec4f,
    // Node: Vector2Input_67
    node_Vector2Input_67_d6ac4dbd: vec4f,
    // Node: Vector2Input_68
    node_Vector2Input_68_87bf4dbd: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;

// --- Extra WGSL declarations (generated) ---
fn mc_math_closure(uv: vec2<f32>, n: vec3<f32>) -> f32 {
    var uv_1: vec2<f32>;
    var n_1: vec3<f32>;
    var output: f32 = 0f;

    uv_1 = uv;
    n_1 = n;
    let _e12: vec3<f32> = n_1;
    output = dot(_e12, vec3<f32>(0f, 0f, 1f));
    let _e18: f32 = output;
    return _e18;
}


// ---- 2D SDF bevel helpers (generated) ----
// 2D SDF bevel helper template.
//
// This file is the editable WGSL source for Sdf2DBevel curve helper functions.
// The Rust compiler wires node inputs into calls to these helpers.

fn sdf2d_bevel_smooth5_map(t_in: f32) -> f32 {
    // Map t in [0, 1] into a symmetric [-1, 1] curve.
    var t = 0.5 + t_in * 0.5;
    t = clamp(t, 0.0, 1.0);
    // 5th-degree smootherstep: t^3 * (t * (t * 6 - 15) + 10)
    t = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    return (t - 0.5) * 2.0;
}

fn sdf2d_bevel_smooth5(d_in: f32, edge: f32, cliff: f32) -> f32 {
    var d = d_in;
    if (d < -edge) {
        d = -edge;
    } else if (d < 0.0) {
        var x = -d / edge;
        if (x >= 0.85) {
            x = 1.0;
        } else {
            x = clamp(x, 0.0, 1.0);
            x = sdf2d_bevel_smooth5_map(pow(x, 0.5));
            x = 1.0 - pow(1.0 - x, cliff);
        }
        d = -x * edge;
    }
    return d;
}

fn sdf2d_bevel_smooth7_map(t_in: f32) -> f32 {
    // Map t in [0, 1] into a symmetric [-1, 1] curve.
    var t = 0.5 + t_in * 0.5;
    t = clamp(t, 0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let t5 = t4 * t;
    let t6 = t5 * t;
    let t7 = t6 * t;
    // 7th-degree smooth polynomial
    t = -20.0 * t7 + 70.0 * t6 - 84.0 * t5 + 35.0 * t4;
    return (t - 0.5) * 2.0;
}

fn sdf2d_bevel_smooth7(d_in: f32, edge: f32, cliff: f32) -> f32 {
    var d = d_in;
    if (d < -edge) {
        d = -edge;
    } else if (d < 0.0) {
        var x = -d / edge;
        if (x >= 0.85) {
            x = 1.0;
        } else {
            x = clamp(x, 0.0, 1.0);
            x = sdf2d_bevel_smooth7_map(pow(x, 0.5));
            x = 1.0 - pow(1.0 - x, cliff);
        }
        d = -x * edge;
    }
    return d;
}

fn sdf2d_bevel_eps() -> f32 {
    return 0.002;
}

fn sdf2d_bevel_normal(depth_px: f32, depth_nx: f32, depth_py: f32, depth_ny: f32, eps: f32) -> vec3f {
    let safe_eps = max(abs(eps), 1e-6);
    let dx = (depth_px - depth_nx) / (2.0 * safe_eps);
    let dy = (depth_py - depth_ny) / (2.0 * safe_eps);
    return normalize(vec3f(-dx, -dy, 1.0));
}

// Note: normal reconstruction uses 4 extra evaluations (finite differences).
// Potential optimization: use `dpdx`/`dpdy` in WGSL to estimate derivatives with fewer calls.


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


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Sdf2DBevel Sdf2DBevel_61.normal finite differences
    let _2d_sdf_bevel_normal_sdf_px = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(sdf2d_bevel_eps(), 0.0)) - (in.geo_size_px * vec2f((graph_inputs.float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.float_input_12).x),
    );
    let _2d_sdf_bevel_normal_sdf_nx = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(-(sdf2d_bevel_eps()), 0.0)) - (in.geo_size_px * vec2f((graph_inputs.float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.float_input_12).x),
    );
    let _2d_sdf_bevel_normal_sdf_py = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(0.0, sdf2d_bevel_eps())) - (in.geo_size_px * vec2f((graph_inputs.float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.float_input_12).x),
    );
    let _2d_sdf_bevel_normal_sdf_ny = sdf2d_round_rect(
        ((in.local_px.xy + vec2f(0.0, -(sdf2d_bevel_eps()))) - (in.geo_size_px * vec2f((graph_inputs.float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.float_input_12).x),
    );
    let _2d_sdf_bevel_normal_depth_px = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_px, 22, 0.46);
    let _2d_sdf_bevel_normal_depth_nx = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_nx, 22, 0.46);
    let _2d_sdf_bevel_normal_depth_py = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_py, 22, 0.46);
    let _2d_sdf_bevel_normal_depth_ny = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_ny, 22, 0.46);
    let _2d_sdf_bevel_normal_normal = sdf2d_bevel_normal(
        _2d_sdf_bevel_normal_depth_px,
        _2d_sdf_bevel_normal_depth_nx,
        _2d_sdf_bevel_normal_depth_py,
        _2d_sdf_bevel_normal_depth_ny,
        sdf2d_bevel_eps(),
    );
    var math_closure_out: f32;
    {
        let n = _2d_sdf_bevel_normal_normal;
        var output: f32;
        output = mc_math_closure(in.uv, n);
        math_closure_out = output;
    }
    // Remap Remap_64.result
    let remap = smoothstep(
        0.0,
        -2.0,
        sdf2d_round_rect((in.local_px.xy - (in.geo_size_px * vec2f((graph_inputs.float_input_10).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.float_input_12).x)),
    );
    // Final composite
    let _frag_out = vec4f((math_closure_out * remap));
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
