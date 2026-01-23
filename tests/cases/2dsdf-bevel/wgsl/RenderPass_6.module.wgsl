
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
};

@group(0) @binding(0)
var<uniform> params: Params;

 struct VSOut {
     @builtin(position) position: vec4f,
     @location(0) uv: vec2f,
     // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
     @location(1) frag_coord_gl: vec2f,
     // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
     @location(2) local_px: vec2f,
     // Geometry size in pixels after applying geometry/instance transforms.
     @location(3) geo_size_px: vec2f,
  };

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;

// --- Extra WGSL declarations (generated) ---
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


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
     @location(2) i0: vec4f,
     @location(3) i1: vec4f,
     @location(4) i2: vec4f,
     @location(5) i3: vec4f,
 ) -> VSOut {
 var out: VSOut;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = uv;

 let inst_m = mat4x4f(i0, i1, i2, i3);
 let geo_sx = length(inst_m[0].xy);
 let geo_sy = length(inst_m[1].xy);
 let geo_size_px = params.geo_size * vec2f(geo_sx, geo_sy);
 out.geo_size_px = geo_size_px;
 out.local_px = uv * geo_size_px;

 var p_local = (inst_m * vec4f(position, 1.0)).xyz;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 let p_px = params.center + p_local.xy;

 // Convert pixels to clip space assuming bottom-left origin.
 // (0,0) => (-1,-1), (target_size) => (1,1)
 let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
 out.position = vec4f(ndc, position.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return vec4f(normalize(vec3f(-(((sdf2d_bevel_smooth7(sdf2d_round_rect(((in.local_px + vec2f(1.0, 0.0)) - (in.geo_size_px * vec2f(0.5))), (in.geo_size_px * 0.5), vec4f(22.0)), 20, 0.5)) - (sdf2d_bevel_smooth7(sdf2d_round_rect(((in.local_px + vec2f(-1.0, 0.0)) - (in.geo_size_px * vec2f(0.5))), (in.geo_size_px * 0.5), vec4f(22.0)), 20, 0.5))) * 0.5), -(((sdf2d_bevel_smooth7(sdf2d_round_rect(((in.local_px + vec2f(0.0, 1.0)) - (in.geo_size_px * vec2f(0.5))), (in.geo_size_px * 0.5), vec4f(22.0)), 20, 0.5)) - (sdf2d_bevel_smooth7(sdf2d_round_rect(((in.local_px + vec2f(0.0, -1.0)) - (in.geo_size_px * vec2f(0.5))), (in.geo_size_px * 0.5), vec4f(22.0)), 20, 0.5))) * 0.5), 1.0)), 0.0);
}
