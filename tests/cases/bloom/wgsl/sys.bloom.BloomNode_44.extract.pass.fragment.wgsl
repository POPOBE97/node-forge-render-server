
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


@group(1) @binding(0)

var src_tex: texture_2d<f32>;
@group(1) @binding(1)
var src_samp: sampler;

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    
let src = textureSample(src_tex, src_samp, in.uv);
let lum = dot(src.rgb, vec3f(0.2126, 0.7152, 0.0722));
let mask = smoothstep(0.00000000, 0.15686275, lum);
let extracted = src.rgb * mask * 1.00000000;
let gray = dot(extracted, vec3f(0.2126, 0.7152, 0.0722));
let sat_rgb = mix(vec3f(gray), extracted, 0.00000000);
let tinted = sat_rgb * vec3f(1.00000000, 0.00000000, 0.00000000);
let alpha = clamp(src.a * mask * 1.00000000 * 1.00000000, 0.0, 1.0);
return vec4f(tinted, alpha);

}
