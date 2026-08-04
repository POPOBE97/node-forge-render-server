
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
    // Node: FloatInput_11
    float_input_11: vec4f,
    // Node: FloatInput_2
    float_input_2: vec4f,
    // Node: FloatInput_3
    float_input_3: vec4f,
    // Node: FloatInput_39
    float_input_39: vec4f,
    // Node: FloatInput_40
    float_input_40: vec4f,
    // Node: FloatInput_43
    float_input_43: vec4f,
    // Node: GroupInstance_102/FloatInput_138
    group_instance_102_float_input_138: vec4f,
    // Node: GroupInstance_38/FloatInput_22
    group_instance_38_float_input_22: vec4f,
    // Node: GroupInstance_38/FloatInput_23
    group_instance_38_float_input_23: vec4f,
    // Node: GroupInstance_38/FloatInput_26
    group_instance_38_float_input_26: vec4f,
    // Node: GroupInstance_38/FloatInput_30
    group_instance_38_float_input_30: vec4f,
    // Node: GroupInstance_38/FloatInput_34
    group_instance_38_float_input_34: vec4f,
    // Node: GroupInstance_42/FloatInput_22
    group_instance_42_float_input_22: vec4f,
    // Node: GroupInstance_42/FloatInput_23
    group_instance_42_float_input_23: vec4f,
    // Node: GroupInstance_42/FloatInput_26
    group_instance_42_float_input_26: vec4f,
    // Node: GroupInstance_42/FloatInput_30
    group_instance_42_float_input_30: vec4f,
    // Node: GroupInstance_42/FloatInput_34
    group_instance_42_float_input_34: vec4f,
    // Node: Vector3Input_12
    vector3_input_12: vec4f,
    // Node: Vector3Input_46
    vector3_input_46: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_GroupInstance_102_ImageTexture_2: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_GroupInstance_102_ImageTexture_2: sampler;


// --- Extra WGSL declarations (generated) ---

fn aspect_correct_uv_fit(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    // r = image_aspect / geo_aspect; r > 1 means image is relatively wider than geometry.
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(max(1.0 / r, 1.0), max(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}
fn aspect_correct_uv_fill(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(min(1.0 / r, 1.0), min(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}

// Group: CalculateLighting
fn calculate_lighting(normal: vec3f, light_dir: vec3f, reflect_angle: f32, intensity: f32) -> f32 {
    // Divide GroupInstance_38/MathDivide_29.result
    let math_divide = (
        acos(clamp(dot(normalize(light_dir), normal), (graph_inputs.group_instance_38_float_input_22).x, (graph_inputs.group_instance_38_float_input_23).x))
        / ((graph_inputs.group_instance_38_float_input_26).x * reflect_angle)
    );
    // Multiply GroupInstance_38/MathMultiply_35.result
    let lighting = (
        max((intensity * ((graph_inputs.group_instance_38_float_input_30).x - math_divide)), (graph_inputs.group_instance_38_float_input_34).x)
        * max(dot(normalize(light_dir), normal), (graph_inputs.group_instance_38_float_input_34).x)
    );
    return lighting;
}


// ---- LuminanceCurve LAB helper (generated) ----
// LuminanceCurve (LAB) — helper template
//
// Embedded at compile time via template_loader. No markers — this file is the
// raw WGSL helper body that gets registered in `extra_wgsl_decls`.
//
// Algorithm:
//   - Convert premultiplied RGB to linear RGB (divide by alpha).
//   - Convert linear RGB to OKLab via the Björn Ottosson matrices.
//   - Apply a cubic Bézier curve (defined by `factors.xyzw` at x = 0, 1/3, 2/3, 1)
//     to the L channel only, then mix between original and curved L by `mix_factor`.
//   - Convert back to linear RGB and re-premultiply by alpha.

fn lc_luminance_curve_lab(color: vec4f, factors: vec4f, mix_factor: f32) -> vec4f {
    let factor_adjust = vec4f(
        -1.0 * factors.x + 3.0 * factors.y - 3.0 * factors.z + 1.0 * factors.w,
        3.0 * factors.x - 6.0 * factors.y + 3.0 * factors.z,
        -3.0 * factors.x + 3.0 * factors.y,
        factors.x,
    );

    if (color.a <= 0.0001) {
        return color;
    }

    let rgb = color.rgb / color.a;
    let lms = vec3f(
        0.4122214708 * rgb.r + 0.5363325363 * rgb.g + 0.0514459929 * rgb.b,
        0.2119034982 * rgb.r + 0.6806995451 * rgb.g + 0.1073969566 * rgb.b,
        0.0883024619 * rgb.r + 0.2817188376 * rgb.g + 0.6299787005 * rgb.b,
    );
    let lms_cbrt = sign(lms) * pow(abs(lms), vec3f(1.0 / 3.0));
    let lab = vec3f(
        0.2104542553 * lms_cbrt.x + 0.7936177850 * lms_cbrt.y - 0.0040720468 * lms_cbrt.z,
        1.9779984951 * lms_cbrt.x - 2.4285922050 * lms_cbrt.y + 0.4505937099 * lms_cbrt.z,
        0.0259040371 * lms_cbrt.x + 0.7827717662 * lms_cbrt.y - 0.8086757660 * lms_cbrt.z,
    );

    let curve_input = clamp(lab.x, 0.0, 1.0);
    var target_l = curve_input * factor_adjust.x + factor_adjust.y;
    target_l = target_l * curve_input + factor_adjust.z;
    target_l = target_l * curve_input + factor_adjust.w;

    let mapped_l = mix(lab.x, target_l, mix_factor);
    let mapped_lms_cbrt = vec3f(
        mapped_l + 0.3963377774 * lab.y + 0.2158037573 * lab.z,
        mapped_l - 0.1055613458 * lab.y - 0.0638541728 * lab.z,
        mapped_l - 0.0894841775 * lab.y - 1.2914855480 * lab.z,
    );
    let mapped_lms = mapped_lms_cbrt * mapped_lms_cbrt * mapped_lms_cbrt;
    var mapped_rgb = vec3f(
        4.0767416621 * mapped_lms.x - 3.3077115913 * mapped_lms.y + 0.2309699292 * mapped_lms.z,
        -1.2684380046 * mapped_lms.x + 2.6097574011 * mapped_lms.y - 0.3413193965 * mapped_lms.z,
        -0.0041960863 * mapped_lms.x - 0.7034186147 * mapped_lms.y + 1.7076147010 * mapped_lms.z,
    );
    mapped_rgb = max(vec3f(0.0), mapped_rgb);

    return vec4f(mapped_rgb * color.a, color.a);
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


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
 ) -> VSOut {
 var out: VSOut;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = vec2f(uv.x, 1.0 - uv.y);

 out.geo_size_px = params.geo_size;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(uv * out.geo_size_px, 0.0);

 var p_local = position;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = params.center + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // ImageTexture GroupInstance_102/ImageTexture_2 aspect-correct uv
    let image_texture_uv = aspect_correct_uv_fill(
        (in.uv),
        vec2f(textureDimensions(img_tex_GroupInstance_102_ImageTexture_2)),
        in.geo_size_px,
    );
    // ImageTexture GroupInstance_102/ImageTexture_2.color
    let image_texture_sample = textureSample(
        img_tex_GroupInstance_102_ImageTexture_2,
        img_samp_GroupInstance_102_ImageTexture_2,
        image_texture_uv,
    );
    // Sdf2DBevel Sdf2DBevel_5.normal finite differences
    let _2d_sdf_bevel_normal_sdf_px = sdf2d_smooth_round_rect(
        abs(((in.local_px.xy + vec2f(sdf2d_bevel_eps(), 0.0)) - vec2f(79.5, 79.5))),
        (vec2f(159, 159) * 0.5),
        36,
        vec2f(0.0, 0.0),
    ).x;
    let _2d_sdf_bevel_normal_sdf_nx = sdf2d_smooth_round_rect(
        abs(((in.local_px.xy + vec2f(-(sdf2d_bevel_eps()), 0.0)) - vec2f(79.5, 79.5))),
        (vec2f(159, 159) * 0.5),
        36,
        vec2f(0.0, 0.0),
    ).x;
    let _2d_sdf_bevel_normal_sdf_py = sdf2d_smooth_round_rect(
        abs(((in.local_px.xy + vec2f(0.0, sdf2d_bevel_eps())) - vec2f(79.5, 79.5))),
        (vec2f(159, 159) * 0.5),
        36,
        vec2f(0.0, 0.0),
    ).x;
    let _2d_sdf_bevel_normal_sdf_ny = sdf2d_smooth_round_rect(
        abs(((in.local_px.xy + vec2f(0.0, -(sdf2d_bevel_eps()))) - vec2f(79.5, 79.5))),
        (vec2f(159, 159) * 0.5),
        36,
        vec2f(0.0, 0.0),
    ).x;
    let _2d_sdf_bevel_normal_depth_px = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_normal_sdf_px,
        (graph_inputs.float_input_2).x,
        (graph_inputs.float_input_11).x,
    );
    let _2d_sdf_bevel_normal_depth_nx = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_normal_sdf_nx,
        (graph_inputs.float_input_2).x,
        (graph_inputs.float_input_11).x,
    );
    let _2d_sdf_bevel_normal_depth_py = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_normal_sdf_py,
        (graph_inputs.float_input_2).x,
        (graph_inputs.float_input_11).x,
    );
    let _2d_sdf_bevel_normal_depth_ny = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_normal_sdf_ny,
        (graph_inputs.float_input_2).x,
        (graph_inputs.float_input_11).x,
    );
    let _2d_sdf_bevel_normal_normal = sdf2d_bevel_normal(
        _2d_sdf_bevel_normal_depth_px,
        _2d_sdf_bevel_normal_depth_nx,
        _2d_sdf_bevel_normal_depth_py,
        _2d_sdf_bevel_normal_depth_ny,
        sdf2d_bevel_eps(),
    );
    // Multiply GroupInstance_42/MathMultiply_35.result
    let lighting = calculate_lighting(
        _2d_sdf_bevel_normal_normal,
        ((graph_inputs.vector3_input_12).xyz * (graph_inputs.vector3_input_46).xyz),
        (graph_inputs.float_input_40).x,
        (graph_inputs.float_input_43).x,
    );
    // Luminance Curve LuminanceCurve_103.color
    let luminance_curve = lc_luminance_curve_lab(
        (vec4f((graph_inputs.group_instance_102_float_input_138).x) * image_texture_sample),
        vec4f(0.701035976, 0.700163722, 1.502358794, 1.499537945),
        (calculate_lighting(_2d_sdf_bevel_normal_normal, (graph_inputs.vector3_input_12).xyz, (graph_inputs.float_input_40).x, (graph_inputs.float_input_39).x) + lighting),
    );
    // Sdf2DBevel Sdf2DBevel_5.depth
    let _2d_sdf_bevel_depth_sdf_depth = sdf2d_smooth_round_rect(
        abs((in.local_px.xy - vec2f(79.5, 79.5))),
        (vec2f(159, 159) * 0.5),
        36,
        vec2f(0.0, 0.0),
    ).x;
    let _2d_sdf_bevel_depth_depth = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_depth_sdf_depth,
        (graph_inputs.float_input_2).x,
        (graph_inputs.float_input_11).x,
    );
    // Sdf2DBevel Sdf2DBevel_7.depth
    let _2d_sdf_bevel_depth_sdf_depth_f1ab2a63 = (graph_inputs.float_input_3).x;
    let _2d_sdf_bevel_depth_depth_1d040fe9 = sdf2d_bevel_smooth5(
        _2d_sdf_bevel_depth_sdf_depth_f1ab2a63,
        (graph_inputs.float_input_2).x,
        (graph_inputs.float_input_11).x,
    );
    // Final composite
    let _frag_out = (luminance_curve * vec4f(smoothstep(0.0, _2d_sdf_bevel_depth_depth_1d040fe9, _2d_sdf_bevel_depth_depth)));
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
