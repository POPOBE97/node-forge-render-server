
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


@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;

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

// Analytic ceramic dial, soft studio backdrop, and cast shadow. LocalUV is Y-up.
fn dial_distance_gauge_surface_material(p: vec2f) -> f32 {
    let q = abs((p - vec2f(360.0, 350.0)) / 324.0);
    let k = 4.5;
    let f = pow(q.x, k) + pow(q.y, k);
    let g = k * length(pow(max(q, vec2f(0.001)), vec2f(k - 1.0))) / 324.0;
    return (f - 1.0) / max(g, 0.0031);
}

fn shader_material_gauge_surface_material(in: ShaderMaterialInput) -> vec4f {
    let p = in.uv * vec2f(716.0, 702.0);
    var bg = mix(vec3f(0.60, 0.54, 0.65), vec3f(1.0, 0.55, 0.59), smoothstep(320.0, 710.0, p.y));
    bg = mix(bg, vec3f(0.57, 0.53, 0.65), 0.15 * (1.0 - p.x / 716.0));
    let shadow = exp(-max(dial_distance_gauge_surface_material(p + vec2f(0.0, 28.0)), 0.0) / 27.0)
        * mix(1.0, 0.18, smoothstep(130.0, 660.0, p.y));
    bg *= 1.0 - 0.63 * shadow;
    let d = dial_distance_gauge_surface_material(p);
    let n = normalize(vec2f(dial_distance_gauge_surface_material(p + vec2f(0.5, 0.0)) - dial_distance_gauge_surface_material(p - vec2f(0.5, 0.0)),
        dial_distance_gauge_surface_material(p + vec2f(0.0, 0.5)) - dial_distance_gauge_surface_material(p - vec2f(0.0, 0.5))) + vec2f(0.00001));
    let top = exp(-dot((p - vec2f(210.0, 660.0)) / vec2f(430.0, 330.0), (p - vec2f(210.0, 660.0)) / vec2f(430.0, 330.0)));
    let teal = exp(-dot((p - vec2f(620.0, 220.0)) / vec2f(350.0, 440.0), (p - vec2f(620.0, 220.0)) / vec2f(350.0, 440.0)));
    var body = vec3f(0.0006, 0.0015, 0.0017) + top * vec3f(0.0090, 0.0092, 0.0091) + teal * vec3f(0.0, 0.0085, 0.0070);
    let light = pow(max(dot(n, normalize(vec2f(-0.45, 0.89))), 0.0), 2.0);
    let rim = exp(-pow((d + 1.6) / 1.4, 2.0)) * 0.061 + exp(-max(-d, 0.0) / 5.5) * 0.026;
    body += rim * light;
    body += exp(-max(-d, 0.0) / 3.4) * pow(max(dot(n, normalize(vec2f(0.8, -0.6))), 0.0), 3.0) * vec3f(0.001, 0.033, 0.026);
    let coverage = 1.0 - smoothstep(-0.7, 0.7, d);
    return vec4f(mix(bg, body, coverage), 1.0);
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
    // Shader Material gauge.surface.material.material
    let shader_material_material = shader_material_gauge_surface_material(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
    );
    // Final composite
    let _frag_out = shader_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
