
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
      @location(6) world_pos: vec3f,
 };


@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_gauge_label10_sdf: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_gauge_label10_sdf: sampler;


// --- Extra WGSL declarations (generated) ---
fn nf_pbr_unit(v: vec3f) -> vec3f {
    return select(vec3f(0.0, 0.0, 1.0), v * inverseSqrt(max(dot(v, v), 1e-12)), dot(v, v) > 1e-12);
}
fn nf_pbr(world: vec3f, local: vec2f, base: vec4f, metallic_in: f32,
    roughness_in: f32, local_normal: vec3f, normal_strength: f32, emission: vec4f,
    alpha: f32, light_direction: vec3f, light_color: vec4f, intensity: f32,
    ambient: vec4f, view_direction: vec3f) -> vec4f {
    // Derivative frame follows each instance's rotation, scale and reflection.
    let qx = dpdx(world); let qy = dpdy(world);
    let sx = dpdx(local); let sy = dpdy(local);
    let det = sx.x * sy.y - sx.y * sy.x;
    let safe_det = select(-max(abs(det), 1e-8), max(abs(det), 1e-8), det >= 0.0);
    let tangent = (qx * sy.y - qy * sx.y) / safe_det;
    let bitangent = (qy * sx.x - qx * sy.x) / safe_det;
    let geometric = nf_pbr_unit(cross(tangent, bitangent));
    // Inverse-transpose of the local frame, so a local SDF normal stays local.
    let n = nf_pbr_unit(cross(bitangent, geometric) * local_normal.x * normal_strength
        + cross(geometric, tangent) * local_normal.y * normal_strength
        + cross(tangent, bitangent) * local_normal.z);
    let l = nf_pbr_unit(light_direction);
    let v = nf_pbr_unit(view_direction);
    let h = nf_pbr_unit(l + v);
    let nl = max(dot(n, l), 0.0);
    let nv = max(dot(n, v), 0.0001);
    let nh = max(dot(n, h), 0.0);
    let vh = max(dot(v, h), 0.0);
    let metallic = clamp(metallic_in, 0.0, 1.0);
    let roughness = clamp(roughness_in, 0.045, 1.0);
    let color = max(base.rgb / max(base.a, 1e-8), vec3f(0.0));
    let f0 = mix(vec3f(0.04), color, metallic);
    let fresnel = f0 + (1.0 - f0) * pow(1.0 - vh, 5.0);
    let a2 = pow(roughness, 4.0);
    let denom = nh * nh * (a2 - 1.0) + 1.0;
    let distribution = a2 / max(3.14159265 * denom * denom, 1e-8);
    let visibility = 0.5 / max(nl * sqrt(nv * nv * (1.0 - a2) + a2)
        + nv * sqrt(nl * nl * (1.0 - a2) + a2), 1e-6);
    let diffuse = (1.0 - fresnel) * (1.0 - metallic) * color / 3.14159265;
    let specular = distribution * visibility * fresnel;
    let radiance = light_color.rgb * max(intensity, 0.0);
    let lit = (diffuse + specular) * radiance * nl
        + ambient.rgb * color * (1.0 - metallic) + emission.rgb;
    let coverage = clamp(alpha * base.a, 0.0, 1.0);
    return vec4f(lit * coverage, coverage);
}


// ---- 2D SDF bevel helpers (generated) ----
// 2D SDF bevel helper template.
//
// This file is the editable WGSL source for Sdf2DBevel curve helper functions.
// The Rust compiler wires node inputs into calls to these helpers.

fn sdf2d_bevel_smooth5_map(t_in: f32) -> f32 {
    // Evaluate the symmetric polynomial in the nearer endpoint's coordinate.
    // Expanded terms near t=1 lose precision before finite-difference normals.
    let t = clamp(0.5 + t_in * 0.5, 0.0, 1.0);
    let q = min(t, 1.0 - t);
    let polynomial = q * q * q * (10.0 + q * (-15.0 + 6.0 * q));
    return select(2.0 * polynomial - 1.0, 1.0 - 2.0 * polynomial, t >= 0.5);
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
    let t = clamp(0.5 + t_in * 0.5, 0.0, 1.0);
    let q = min(t, 1.0 - t);
    let q2 = q * q;
    let polynomial = q2 * q2 * (35.0 + q * (-84.0 + q * (70.0 - 20.0 * q)));
    return select(2.0 * polynomial - 1.0, 1.0 - 2.0 * polynomial, t >= 0.5);
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

// Private texture storage is top-down; the node boundary is bottom-left LocalPx.
// Manual bilinear sampling also works on devices without FLOAT32_FILTERABLE.
fn nf_text_sdf(field: texture_2d<f32>, local_px: vec2f) -> f32 {
    let metadata = textureLoad(field, vec2i(0), 0).gba;
    let dims = vec2i(textureDimensions(field));
    let raster = vec2f(local_px.x, metadata.y - local_px.y) * metadata.z - 0.5;
    let bounded = clamp(raster, vec2f(0.0), vec2f(dims - 1));
    let lo = vec2i(floor(bounded));
    let hi = min(lo + 1, dims - 1);
    let t = fract(bounded);
    let a = mix(textureLoad(field, lo, 0).r, textureLoad(field, vec2i(hi.x, lo.y), 0).r, t.x);
    let b = mix(textureLoad(field, vec2i(lo.x, hi.y), 0).r, textureLoad(field, hi, 0).r, t.x);
    let d = mix(a, b, t.y);
    let outside = length(raster - bounded) / metadata.z;
    return select(d, max(d, 0.0) + outside, outside > 0.0);
}
fn nf_sdf_coverage(distance: f32) -> f32 {
    let aa = max(fwidth(distance), 0.0001);
    return clamp(0.5 - distance / aa, 0.0, 1.0);
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

 out.world_pos = vec3f(p_px, p_local.z);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Sdf2DBevel gauge.label10.bevel.normal finite differences
    let _2d_sdf_bevel_normal_sdf_px = nf_text_sdf(
        img_tex_gauge_label10_sdf,
        (in.local_px.xy + vec2f(max(abs(0.35), 0.001), 0.0)) - (vec2f(0, 0)),
    );
    let _2d_sdf_bevel_normal_sdf_nx = nf_text_sdf(
        img_tex_gauge_label10_sdf,
        (in.local_px.xy + vec2f(-(max(abs(0.35), 0.001)), 0.0)) - (vec2f(0, 0)),
    );
    let _2d_sdf_bevel_normal_sdf_py = nf_text_sdf(
        img_tex_gauge_label10_sdf,
        (in.local_px.xy + vec2f(0.0, max(abs(0.35), 0.001))) - (vec2f(0, 0)),
    );
    let _2d_sdf_bevel_normal_sdf_ny = nf_text_sdf(
        img_tex_gauge_label10_sdf,
        (in.local_px.xy + vec2f(0.0, -(max(abs(0.35), 0.001)))) - (vec2f(0, 0)),
    );
    let _2d_sdf_bevel_normal_depth_px = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_px, 4, 0.8);
    let _2d_sdf_bevel_normal_depth_nx = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_nx, 4, 0.8);
    let _2d_sdf_bevel_normal_depth_py = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_py, 4, 0.8);
    let _2d_sdf_bevel_normal_depth_ny = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_ny, 4, 0.8);
    let _2d_sdf_bevel_normal_normal = sdf2d_bevel_normal(
        _2d_sdf_bevel_normal_depth_px,
        _2d_sdf_bevel_normal_depth_nx,
        _2d_sdf_bevel_normal_depth_py,
        _2d_sdf_bevel_normal_depth_ny,
        max(abs(0.35), 0.001),
    );
    // PBR Material gauge.label10.material.material
    let pbr_material_material = nf_pbr(
        in.world_pos,
        in.local_px.xy,
        vec4f(0.001, 0.180000007, 0.180000007, 1.0),
        0.119999997,
        0.5,
        _2d_sdf_bevel_normal_normal,
        0.699999988,
        vec4f(0.0, 0.699999988, 0.680000007, 1.0),
        nf_sdf_coverage(nf_text_sdf(img_tex_gauge_label10_sdf, in.local_px.xy - (vec2f(0, 0)))),
        vec3f(5.125999928, -5.940000057, 1.0),
        vec4f(1.0, 1.0, 1.0, 1.0),
        8.0,
        vec4f(0.129999995, 0.159999996, 0.170000002, 1.0),
        vec3f(0.0, 0.0, 1.0),
    );
    // Final composite
    let _frag_out = pbr_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
