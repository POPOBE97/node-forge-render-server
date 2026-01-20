
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
@group(1) @binding(0)
var pass_tex___auto_fullscreen_pass__edge_65: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp___auto_fullscreen_pass__edge_65: sampler;

@group(1) @binding(2)
var pass_tex___auto_fullscreen_pass__edge_66: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp___auto_fullscreen_pass__edge_66: sampler;

@group(1) @binding(4)
var pass_tex___auto_fullscreen_pass__edge_67: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp___auto_fullscreen_pass__edge_67: sampler;

@group(1) @binding(6)
var pass_tex___auto_fullscreen_pass__edge_68: texture_2d<f32>;

@group(1) @binding(7)
var pass_samp___auto_fullscreen_pass__edge_68: sampler;


// --- Extra WGSL declarations (generated) ---

// ---- GlassMaterial helpers (generated) ----

fn glass_luma(color: vec3f) -> f32 {
    return dot(color, vec3f(0.2126, 0.7152, 0.0722));
}

fn glass_rgb2hsv(c: vec3f) -> vec3f {
    let K = vec4f(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = select(vec4f(c.bg, K.wz), vec4f(c.gb, K.xy), c.b < c.g);
    let q = select(vec4f(p.xyw, c.r), vec4f(c.r, p.yzx), p.x < c.r);
    let d = q.x - min(q.w, q.y);
    let e = 1e-10;
    return vec3f(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

fn glass_hsv2rgb(c: vec3f) -> vec3f {
    let K = vec4f(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, vec3f(0.0), vec3f(1.0)), c.y);
}

fn glass_adjust_color(color: vec4f, saturation: f32, brightness: f32) -> vec4f {
    let luminance = dot(color.rgb, vec3f(0.2125, 0.7153, 0.0721));
    let adjusted_sat = saturation * color.rgb + (1.0 - saturation) * vec3f(luminance);
    let a = color.a;
    let adjusted_bright = adjusted_sat + vec3f(brightness * a);
    return vec4f(adjusted_bright, a);
}

fn glass_luminance_curve(color: vec4f, factors: vec4f, mix_factor: f32) -> vec4f {
    // GLSL mat4 * vec4 factors, expanded in WGSL.
    // adjustment_matrix:
    // -1  3 -3  1
    //  3 -6  3  0
    // -3  3  0  0
    //  1  0  0  0
    let factor_adjust = vec4f(
        -1.0 * factors.x + 3.0 * factors.y + -3.0 * factors.z + 1.0 * factors.w,
        3.0 * factors.x + -6.0 * factors.y + 3.0 * factors.z + 0.0 * factors.w,
        -3.0 * factors.x + 3.0 * factors.y + 0.0 * factors.z + 0.0 * factors.w,
        1.0 * factors.x + 0.0 * factors.y + 0.0 * factors.z + 0.0 * factors.w
    );

    let alpha = max(color.a, 0.0001);
    let scale = 1.0 / alpha;
    let scaled_rgb = scale * color.rgb;
    var luminance = dot(scaled_rgb, vec3f(0.2125, 0.7153, 0.0721));
    luminance = clamp(luminance, 0.0, 1.0);

    var adj = luminance * factor_adjust.x + factor_adjust.y;
    adj = adj * luminance + factor_adjust.z;
    adj = adj * luminance + factor_adjust.w;
    adj = clamp(adj, 0.0, 1.0);

    let mixed = mix(scaled_rgb, vec3f(adj), mix_factor);
    let result_rgb = mixed * alpha;
    return vec4f(result_rgb, color.a);
}

fn glass_process_color(color: vec4f, luminance_values: vec4f, luminance_amount: f32, saturation: f32, brightness: f32) -> vec4f {
    var c = glass_luminance_curve(color, luminance_values, luminance_amount);
    c = vec4f(glass_adjust_color(c, saturation, brightness).rgb, c.a);
    return c;
}

// Edge curve approximation from the existing glass test graphs (smooth7_vertical with k=0.5).
fn glass_smooth7_vertical(x: f32, k: f32) -> f32 {
    var t = pow(clamp(x, 0.0, 1.0), k);
    t = mix(0.5, 1.0, t);
    t = clamp(t, 0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let t5 = t4 * t;
    let t6 = t5 * t;
    let t7 = t6 * t;
    t = -20.0 * t7 + 70.0 * t6 - 84.0 * t5 + 35.0 * t4;
    t = (t - 0.5) * 2.0;
    return t;
}

fn glass_curve(x: f32, pow_ratio: f32) -> f32 {
    if (x >= 0.85) {
        return 1.0;
    }
    let circle = glass_smooth7_vertical(x, 0.5);
    let circle_pow = 1.0 - pow(1.0 - circle, pow_ratio);
    return circle_pow;
}

fn glass_box_sdf(p: vec2f, b: vec2f, r: f32) -> f32 {
    let d = abs(p) - b + vec2f(r);
    return min(max(d.x, d.y), 0.0) + length(max(d, vec2f(0.0))) - r;
}

fn glass_shape_sdf(p: vec2f, b: vec2f, r: f32, edge: f32, edge_pow: f32) -> f32 {
    var d = glass_box_sdf(p, b, r);
    if (d < -edge) {
        d = -edge;
    } else if (d < 0.0) {
        let per = (-d / edge);
        let per2 = glass_curve(per, edge_pow);
        d = -per2 * edge;
    }
    return d;
}

fn glass_calculate_normal(pos_from_center: vec2f, half_size_px: vec2f, radius_px: f32, edge: f32, edge_pow: f32) -> vec3f {
    let eps = 1.0;
    let right_sdf = glass_shape_sdf(pos_from_center + vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let left_sdf = glass_shape_sdf(pos_from_center - vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let top_sdf = glass_shape_sdf(pos_from_center + vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let bottom_sdf = glass_shape_sdf(pos_from_center - vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let xy_grad = vec2f((right_sdf - left_sdf) * 0.5, (top_sdf - bottom_sdf) * 0.5);
    return normalize(vec3f(xy_grad, 1.0));
}

fn glass_hsvv(col: vec3f, lighten: f32) -> vec3f {
    let v = glass_luma(col);
    let w = smoothstep(0.0, 0.5, v);
    let k = mix(1.0 - v, v, w);
    let g = 1.0 + smoothstep(0.0, 1.0, lighten) * mix(0.75, 0.4, w);
    return (col + vec3f(k)) * g - vec3f(k);
}

fn glass_dynamic_add(color: vec3f) -> f32 {
    var white_dis = distance(vec3f(1.0), color);
    white_dis = smoothstep(0.2, 1.0, white_dis);
    white_dis = mix(0.5, 1.0, white_dis);
    let lumin = glass_luma(color);
    return lumin * white_dis;
}

fn glass_add_light(color: vec3f, light_color: vec3f, light_strength: f32) -> vec3f {
    var white_dis = distance(vec3f(1.0), color);
    white_dis = smoothstep(0.2, 1.0, white_dis);
    white_dis = mix(0.3, 1.0, white_dis);
    let s = light_strength * white_dis;
    return color + light_color * s;
}

fn glass_calculate_lighting(normal: vec3f, light_dir: vec3f, intensity: f32, angle_range: f32) -> f32 {
    let nld = normalize(light_dir);
    let dp = dot(normal, nld);
    let reflection_angle = acos(clamp(dp, -1.0, 1.0));
    let angle_factor = 1.0 - (reflection_angle / (3.14159 * angle_range));
    let adjusted = max(intensity * angle_factor, 0.0);
    return max(dp, 0.0) * adjusted;
}

fn glass_texture_map(
    tex: texture_2d<f32>,
    samp: sampler,
    uv: vec2f,
    is_bg: bool,
    darker: f32,
    darker_range: vec2f,
    fg_tex: texture_2d<f32>,
    fg_samp: sampler,
) -> vec4f {
    // Pass textures use WGSL texture coordinates with (0,0) at top-left.
    // Our renderer's UV convention is bottom-left, so we flip Y here.
    let uv2 = vec2f(uv.x, 1.0 - uv.y);
    var col = textureSample(tex, samp, uv2);

    if (is_bg) {
        let lum = glass_luma(col.rgb);
        let dark = mix(0.0, darker, smoothstep(darker_range.x, darker_range.y, lum));
        col = vec4f(mix(col.rgb, vec3f(0.0), dark), col.a);
    }

    let fg_col = textureSample(fg_tex, fg_samp, uv2);
    let lighten = fg_col.r;
    col = vec4f(glass_hsvv(col.rgb, lighten), col.a);
    return col;
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