
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
@group(1) @binding(0)
var img_tex_ImageTexture_2: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_ImageTexture_2: sampler;


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
 out.uv = uv;

 out.geo_size_px = params.geo_size;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, 0.0);

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
    // ImageTexture ImageTexture_2 aspect-correct uv
    let image_texture_uv = aspect_correct_uv_fill(
        (in.uv),
        vec2f(textureDimensions(img_tex_ImageTexture_2)),
        in.geo_size_px,
    );
    // ImageTexture ImageTexture_2.color
    let image_texture_sample = textureSample(img_tex_ImageTexture_2, img_samp_ImageTexture_2, image_texture_uv);
    // Final composite
    let _frag_out = image_texture_sample;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
