
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
var img_tex_ImageTexture_9: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_ImageTexture_9: sampler;


// --- Extra WGSL declarations (generated) ---
fn nf_premultiply(c: vec4f) -> vec4f {
    return vec4f(c.rgb * c.a, c.a);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return nf_premultiply(textureSample(img_tex_ImageTexture_9, img_samp_ImageTexture_9, (in.uv)));
}
