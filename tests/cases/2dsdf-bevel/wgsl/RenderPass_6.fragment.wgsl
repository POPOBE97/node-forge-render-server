
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
    node_FloatInput_10_157c0221: vec4f,
    // Node: FloatInput_12
    node_FloatInput_12_af780221: vec4f,
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
fn mc_MathClosure_63_(uv: vec2<f32>, n: vec3<f32>) -> f32 {
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
            x = sdf2d_bevel_smooth5_map(x);
            x = pow(x, cliff);
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
            x = sdf2d_bevel_smooth7_map(x);
            x = pow(x, cliff);
        }
        d = -x * edge;
    }
    return d;
}

// Note: normal reconstruction below uses 4 extra evaluations (finite differences).
// Potential optimization: use `dpdx`/`dpdy` in WGSL to estimate derivatives with fewer calls.

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


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_63_out: f32;
    {
        let n = normalize(vec3f(-(((sdf2d_bevel_smooth7(sdf2d_round_rect(((in.local_px.xy + vec2f(1.0, 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_FloatInput_10_157c0221).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_FloatInput_12_af780221).x)), 22, 0.46)) - (sdf2d_bevel_smooth7(sdf2d_round_rect(((in.local_px.xy + vec2f(-1.0, 0.0)) - (in.geo_size_px * vec2f((graph_inputs.node_FloatInput_10_157c0221).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_FloatInput_12_af780221).x)), 22, 0.46))) * 0.5), -(((sdf2d_bevel_smooth7(sdf2d_round_rect(((in.local_px.xy + vec2f(0.0, 1.0)) - (in.geo_size_px * vec2f((graph_inputs.node_FloatInput_10_157c0221).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_FloatInput_12_af780221).x)), 22, 0.46)) - (sdf2d_bevel_smooth7(sdf2d_round_rect(((in.local_px.xy + vec2f(0.0, -1.0)) - (in.geo_size_px * vec2f((graph_inputs.node_FloatInput_10_157c0221).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_FloatInput_12_af780221).x)), 22, 0.46))) * 0.5), 1.0));
        var output: f32;
        output = mc_MathClosure_63_(in.uv, n);
        mc_MathClosure_63_out = output;
    }
    return vec4f((mc_MathClosure_63_out * smoothstep(0.0, -2.0, sdf2d_round_rect((in.local_px.xy - (in.geo_size_px * vec2f((graph_inputs.node_FloatInput_10_157c0221).x))), (in.geo_size_px * 0.5), vec4f((graph_inputs.node_FloatInput_12_af780221).x)))));
}
