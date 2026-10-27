
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
    // Node: Vector2Input_BackgroundPositionPx
    node_Vector2Input_BackgroundPositionPx_5e1cdc88: vec4f,
    // Node: Vector2Input_BackgroundSizePx
    node_Vector2Input_BackgroundSizePx_187f703e: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_web_system_chrome: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_web_system_chrome: sampler;

@group(1) @binding(2)
var pass_tex_node_default_composite: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_node_default_composite: sampler;

@group(1) @binding(4)
var pass_tex_web_glow_composite: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp_web_glow_composite: sampler;


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

// SOURCE: Web shaders.ts presentShader and browser premultiplied sRGB composition.
// Light + voice bars share one linear F16 layer, then get one hue-preserving SDR knee.
fn encode_srgb_web_present_material(value: vec3f) -> vec3f {
    let v = max(value, vec3f(0.0));
    return select(1.055 * pow(v, vec3f(1.0 / 2.4)) - 0.055, v * 12.92,
        v <= vec3f(0.0031308));
}

fn decode_srgb_web_present_material(value: vec3f) -> vec3f {
    return select(pow((value + 0.055) / 1.055, vec3f(2.4)), value / 12.92,
        value <= vec3f(0.04045));
}

fn limit_highlights_web_present_material(straight: vec3f) -> vec3f {
    let peak = max(max(straight.r, straight.g), straight.b);
    if (peak <= 0.75) { return straight; }
    let mapped = 0.75 + 0.25 * (1.0 - exp(-(peak - 0.75) / 0.25));
    return straight * (mapped / peak);
}

fn shader_material_web_present_material(
    in: ShaderMaterialInput,
    backdrop: texture_2d<f32>, backdrop_sampler: sampler,
    glow: texture_2d<f32>, glow_sampler: sampler,
    chrome: texture_2d<f32>, chrome_sampler: sampler,
) -> vec4f {
    let bg = sample_texture_local_uv(backdrop, backdrop_sampler, in.uv);
    let fg = sample_texture_local_uv(glow, glow_sampler, in.uv);
    let a = clamp(fg.a, 0.0, 1.0);
    let straight = limit_highlights_web_present_material(max(fg.rgb / max(a, 0.00001), vec3f(0.0)));
    let encoded_effect = encode_srgb_web_present_material(straight) * a
        + encode_srgb_web_present_material(bg.rgb / max(bg.a, 0.00001)) * bg.a * (1.0 - a);
    // Anchor the source image's top and bottom chrome without stretching its glyphs.
    let source_size = vec2f(textureDimensions(chrome));
    let scale = in.geometry_size.x / source_size.x;
    let source_y = select(in.uv.y * in.geometry_size.y / scale,
        source_size.y - (1.0 - in.uv.y) * in.geometry_size.y / scale, in.uv.y >= 0.5);
    let system = sample_texture_local_uv(chrome, chrome_sampler, vec2f(in.uv.x, source_y / source_size.y));
    let encoded = encode_srgb_web_present_material(system.rgb / max(system.a, 0.00001)) * system.a
        + encoded_effect * (1.0 - system.a);
    let alpha = a + bg.a * (1.0 - a);
    return vec4f(decode_srgb_web_present_material(encoded / max(alpha, 0.00001)) * alpha, alpha);
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

 let rect_size_px_base = (graph_inputs.node_Vector2Input_BackgroundSizePx_187f703e).xy;
 let rect_center_px = (graph_inputs.node_Vector2Input_BackgroundPositionPx_5e1cdc88).xy;
 let rect_dyn = vec4f(rect_center_px, rect_size_px_base);
 out.geo_size_px = rect_dyn.zw;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(uv * out.geo_size_px, 0.0);

 let p_rect_local_px = vec3f(position.xy * rect_dyn.zw, position.z);
 var p_local = p_rect_local_px;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = rect_dyn.xy + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }